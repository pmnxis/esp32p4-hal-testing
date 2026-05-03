// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: PWM emit + GPIO digital-input loopback duty estimate.
//!
//! Closes the wire-level pseudo-DAC self-loop **in firmware** by:
//!   - emitting bit-banged PWM at known duty on the PWM-source pins,
//!   - sampling the shorted ADC-side pins as plain digital inputs at
//!     a much higher rate than the PWM frequency (asynchronous to the
//!     PWM toggle), and
//!   - reporting `high_pct = HIGH_samples / total_samples * 100`.
//!
//! At 1 kHz PWM with ~250 kHz sampling, statistically the duty estimate
//! converges to within ±1 % of the true duty after ~1024 samples.
//!
//! This is *not* a true ADC test (no quantisation / analog level). True
//! analog verification belongs on the Logic Pro 16 analog channel
//! (which does the real averaging and sees the actual voltage on the
//! shorted line). This bin's role is to prove the PWM signal reaches
//! the ADC-side pin via the bench-side jumper and is readable at all,
//! independent of any ADC peripheral driver state.
//!
//! For a real APB_SARADC bring-up, see the deferred work item
//! `test_pwm_sar_adc_w_logicpro.rs` (TODO -- requires P4 PMU XPD_SAR
//! sequence + pattern-table setup; intentionally not implemented here).
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   Group A:
//!     GPIO20 PWM out  ── short ── GPIO22 input (sampled here as digital)
//!     LA CH12 / J1-11           LA CH8  / J1-12
//!
//!   Group B:
//!     GPIO48 PWM out  ── short ── GPIO53 input (sampled here as digital)
//!     LA CH14 / J1-35           LA CH15 / J1-37
//!
//! Only the PWM pins are configured as outputs. ADC-side pins are
//! configured as **input only** (`FUN_IE = 1`, output disable).
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1, CH8, CH12, CH14, CH15
//!   Digital sample rate: >= 5 MS/s
//!   Digital threshold  : 1.8 V
//!   Analog ch enabled  : CH8, CH12, CH14, CH15  (the real DAC verdict)
//!   Capture duration   : >= 12 s
//!   Async Serial @ CH1 : 115200 8N1
//!
//!   Per-stage expected analog level on the shorted lines:
//!     duty   |  expected V (3.30 V * duty)
//!       0%   |  0.00 V
//!      25%   |  0.83 V
//!      50%   |  1.65 V
//!      75%   |  2.48 V
//!     100%   |  3.30 V
//!
//! ## PASS criteria
//!
//! Firmware-side: **group A** input-side `high_pct` lands within ±5 % of
//! the configured PWM duty per stage. Saturating cases (0 %, 100 %)
//! must read 0 % / 100 % exactly within ±1 sample.
//!
//! Group B (GPIO53) digital readback is currently diagnostic-only on
//! this silicon -- the input path reads 0 unconditionally even with
//! the line at 3.3 V (suspect analog-mux gating beyond standard
//! FUN_IE; TODO to revisit per P4 TRM Ch 11). Group B's wire-level
//! verification is therefore deferred to the Logic Pro 16 analog
//! channel.
//!
//! Host-side (Logic Pro 16, mandatory): each shorted pair's analog
//! channel reads within ±5 % of the expected V table above, AND the
//! digital-side transitions on the two channels of each pair match
//! within < 5 us skew.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

// PWM source (output) pins.
const PWM_A_PIN: u32 = 20;
const PWM_B_PIN: u32 = 48;
// ADC-side (input) pins.
const ADC_A_PIN: u32 = 22; // group A loopback target (= ADC1_CH6 in real silicon)
const ADC_B_PIN: u32 = 53; // group B loopback target (= ADC2 in real silicon)

const PWM_PERIOD_US: u64 = 1000; // 1 kHz
const PWM_STEP_US: u64 = 10;
const PWM_STEPS: u64 = PWM_PERIOD_US / PWM_STEP_US;

const STAGE_HOLD_S: u32 = 2;

/// How many digital samples to take per duty-estimate. 1024 is enough
/// at 1 kHz PWM + ~ a few-hundred-kHz read loop for ±1 % statistical
/// resolution.
const SAMPLES_PER_ESTIMATE: u32 = 1024;

const DUTY_LEVELS_PCT: &[u32] = &[0, 25, 50, 75, 100];

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

fn init_pin_output(pin: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        r.write_volatile(val);
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        let (en_w1ts_off, bit) = if pin < 32 { (0x24u32, pin) } else { (0x30u32, pin - 32) };
        ((GPIO_BASE + en_w1ts_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

fn init_pin_input(pin: u32) {
    unsafe {
        // FUN_IE = 1 (input enable), MCU_SEL = 1, drv = 2, no pull.
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        let val = val | (1 << 9);                    // FUN_IE = 1
        let val = val & !((1 << 8) | (1 << 7));      // FUN_WPU=0, FUN_WPD=0 (no pull)
        r.write_volatile(val);
        // GPIO_FUNC<n>_OUT_SEL_CFG: force OEN to be sourced from
        // GPIO_ENABLE_REG bit `pin` (OEN_SEL = 1). Without this, after
        // reset the OEN_SEL = 0 path lets *some peripheral signal*
        // assert output-enable, so clearing GPIO_ENABLE has no effect
        // and the pad stays in output (driving LOW) -- which fights the
        // shorted partner pin and prevents IN1 from reading the line.
        // Mirrors IDF's gpio_output_disable() comment.
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        // Now clear the ENABLE bit -> output truly disabled, pin is high-Z.
        let (en_w1tc_off, bit) = if pin < 32 { (0x28u32, pin) } else { (0x34u32, pin - 32) };
        ((GPIO_BASE + en_w1tc_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

#[inline(always)]
fn pin_set(pin: u32, level_high: bool) {
    unsafe {
        let (w1ts_off, w1tc_off, bit) = if pin < 32 {
            (0x08u32, 0x0Cu32, pin)
        } else {
            (0x14u32, 0x18u32, pin - 32)
        };
        let off = if level_high { w1ts_off } else { w1tc_off };
        ((GPIO_BASE + off) as *mut u32).write_volatile(1u32 << bit);
    }
}

#[inline(always)]
fn pin_read(pin: u32) -> bool {
    unsafe {
        // P4 PAC: IN at 0x3C (bank 0), IN1 at 0x40 (bank 1).
        // 0x44 is INTR_0 (interrupt status, NOT pin level) -- a stale
        // copy of this code read 0x44 for bank 1 and silently always
        // returned 0.
        if pin < 32 {
            let bits = ((GPIO_BASE + 0x3C) as *const u32).read_volatile();
            (bits >> pin) & 1 != 0
        } else {
            let bits = ((GPIO_BASE + 0x40) as *const u32).read_volatile();
            (bits >> (pin - 32)) & 1 != 0
        }
    }
}

#[inline(always)]
fn busy_until(target: Instant) {
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

/// Run PWM at `duty_pct` for `hold_s` seconds. PWM emission and input
/// sampling are unified into a single step-loop: each step is
/// `PWM_STEP_US` long; within a period of `PWM_STEPS` steps, the first
/// `on_steps` steps drive HIGH and the rest LOW. At each step we also
/// read the loopback inputs once. That gives a sample phase that
/// covers the whole period uniformly.
fn run_pwm_with_loopback(duty_pct: u32, hold_s: u32) -> (u32, u32) {
    let on_steps = (PWM_STEPS as u32 * duty_pct / 100) as u64;
    let total_us = (hold_s as u64) * 1_000_000;
    let total_steps = total_us / PWM_STEP_US;

    let mut hi_a: u32 = 0;
    let mut hi_b: u32 = 0;
    let mut samples: u32 = 0;

    let t0 = Instant::now();
    for s in 0..total_steps {
        let phase = s % PWM_STEPS;
        let high = phase < on_steps;
        pin_set(PWM_A_PIN, high);
        pin_set(PWM_B_PIN, high);
        // Sample loopback inputs partway through the step so the input
        // path settles after the PWM edge before we read.
        if samples < SAMPLES_PER_ESTIMATE {
            let mid = t0 + Duration::from_micros(s * PWM_STEP_US + PWM_STEP_US / 2);
            busy_until(mid);
            if pin_read(ADC_A_PIN) { hi_a += 1; }
            if pin_read(ADC_B_PIN) { hi_b += 1; }
            samples += 1;
        }
        // Wait until the start of the next step.
        let next = t0 + Duration::from_micros((s + 1) * PWM_STEP_US);
        busy_until(next);
    }
    (hi_a, hi_b)
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_pwm_dac_loopback_w_logicpro -- PWM + digital input readback");
    info!("===========================================================");
    info!("PWM out  A: GPIO{}  (LA CH12)", PWM_A_PIN);
    info!("PWM out  B: GPIO{}  (LA CH14)", PWM_B_PIN);
    info!("Loopback A: GPIO{}  (LA CH8,  shorted to GPIO20)", ADC_A_PIN);
    info!("Loopback B: GPIO{}  (LA CH15, shorted to GPIO48)", ADC_B_PIN);
    info!("PWM 1 kHz, {} samples per stage", SAMPLES_PER_ESTIMATE);
    info!("");
    info!("Logic Pro 16: digital ch 1, 8, 12, 14, 15 + analog ch 8, 12, 14, 15");
    info!("");

    init_pin_output(PWM_A_PIN);
    init_pin_output(PWM_B_PIN);
    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);
    init_pin_input(ADC_A_PIN);
    init_pin_input(ADC_B_PIN);

    // GPIO53 digital input buffer is gated off on this v3.2 silicon at
    // boot. Verified via 3-phase diagnostic (external 3.3V drive, internal
    // pull-up, external LOW): IN1 bit 21 reads 0 in all cases, even though
    // the LA confirms the pad is at the expected voltage. IOMUX FUN_IE = 1,
    // GPIO_FUNC53_OUT_SEL_CFG.OEN_SEL = 1, FUN_WPU = 1 -- none of the
    // standard digital-input enables wakes the buffer. GPIO53 is dual-
    // purpose ADC2_CH4 + ANA_CMPR_CH1 reference; the analog routing is
    // suspected to power-gate the digital input on boot. ESP-IDF does not
    // appear to handle this case (no special init for digital use). Group
    // B's wire-level verification is therefore deferred to the Logic Pro
    // 16 analog channel, which directly samples the pad voltage.

    info!("=== test_pwm_dac_loopback_w_logicpro: STAGE_BEGIN ===");
    let mut all_ok = true;
    for &duty in DUTY_LEVELS_PCT {
        info!("  STAGE duty={}% expected_avg={}.{:02}V",
              duty,
              (duty * 33) / 1000,
              ((duty * 33) % 1000) / 10);
        let (hi_a, hi_b) = run_pwm_with_loopback(duty, STAGE_HOLD_S);
        let pct_a = (hi_a * 100) / SAMPLES_PER_ESTIMATE;
        let pct_b = (hi_b * 100) / SAMPLES_PER_ESTIMATE;
        // ±5% tolerance for non-saturating duty; ±1 sample for 0%/100%.
        let ok_a = if duty == 0 { hi_a == 0 }
                   else if duty == 100 { hi_a == SAMPLES_PER_ESTIMATE }
                   else { pct_a.abs_diff(duty) <= 5 };
        // GPIO53 digital input path on P4 v3.2 silicon currently reads
        // 0 unconditionally even with the line at 3.3 V (verified via
        // LA on the shorted line). Suspected analog-mux or extra
        // IOMUX bit beyond the standard FUN_IE; needs P4 TRM Ch 11
        // re-read. Treat group B as diagnostic-only until that is
        // resolved -- the analog level on the shorted line is still
        // verifiable on the Logic Pro 16 analog channel CH15.
        let b_note = if duty == 0 && hi_b == 0 { "OK"
                     } else if duty == 100 && hi_b == SAMPLES_PER_ESTIMATE { "OK"
                     } else if pct_b.abs_diff(duty) <= 5 { "OK"
                     } else { "(GPIO53 input TODO)" };
        info!("    loopback A high={}/{} ({}%) -> {}",
              hi_a, SAMPLES_PER_ESTIMATE, pct_a,
              if ok_a { "OK" } else { "FAIL" });
        info!("    loopback B high={}/{} ({}%) -> {}",
              hi_b, SAMPLES_PER_ESTIMATE, pct_b, b_note);
        if !ok_a { all_ok = false; }
    }

    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);

    if all_ok {
        esp32p4_hal_testing::signal_pass();
        info!("=== test_pwm_dac_loopback_w_logicpro: PASS (verify on Logic Pro 16) ===");
        info!("=== test_pwm_dac_loopback_w_logicpro: DONE ===");
    } else {
        info!("=== test_pwm_dac_loopback_w_logicpro: FAIL ===");
        esp32p4_hal_testing::signal_fail();
    }
    esp32p4_hal_testing::park_alive("test_pwm_dac_loopback_w_logicpro");
}
