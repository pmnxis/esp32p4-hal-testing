// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: pseudo-DAC PWM emission for Logic Pro 16 verification.
//!
//! Step 1 of the LEDC + APB_SARADC verification path. This bin emits
//! deterministic PWM on the two shorted pseudo-DAC pairs and logs
//! the *expected* duty per stage. Host-side Logic Pro 16 capture +
//! analog-mode averaging confirms the analog level on each pair.
//!
//! ADC readback (closing the self-loop in firmware) is intentionally
//! NOT included here -- that is step 2 (`test_pwm_dac_adc_loop_w_logicpro`)
//! once we wire up APB_SARADC raw register access. This bin is the
//! simpler "PWM came out the pin" check first.
//!
//! ## Wiring (la_channel_map.csv)
//!
//! Two physically shorted pairs:
//!
//!   Group A (LA CH8 + CH12, J1 pin 12 + 11):
//!     GPIO20 (PWM out)  ─── short ─── GPIO22 (ADC1_CH6 input)
//!
//!   Group B (LA CH14 + CH15, J1 pin 35 + 37):
//!     GPIO48 (PWM out)  ─── short ─── GPIO53 (ADC2 input)
//!
//! Only the PWM pin is configured as output. ADC pins are left as
//! input-only -- never drive both ends of the short, that's a fight.
//!
//! ## Logic Pro 16 setup
//!
//!   Digital channels enabled  : CH1, CH8, CH12, CH14, CH15
//!   Digital sample rate       : 5 MS/s   (PWM is 1 kHz, plenty of margin)
//!   Digital threshold         : 1.8 V
//!   Analog channels enabled   : CH8, CH12, CH14, CH15
//!   Capture duration          : >= 12 s  (5 stages * 2 s + 2 s margin)
//!   Async Serial analyzer     : CH1 @ 115200 8N1 (verdict text)
//!
//!   Expected on each LA channel pair (CH8 ↔ CH12 and CH14 ↔ CH15):
//!     -- digital edges identical (within ~1 us) on both sides
//!     -- analog avg follows the duty cycle:
//!         duty   |  expected analog avg
//!           0%   |    0.00 V
//!          25%   |    0.83 V
//!          50%   |    1.65 V
//!          75%   |    2.48 V
//!         100%   |    3.30 V
//!
//! ## PWM characteristics
//!
//!   Frequency : 1 kHz  (1000 us period)
//!   Resolution: 1 %    (10 us step)
//!   Method    : bit-bang via direct GPIO write_volatile.
//!               LEDC peripheral driver is not available on P4 yet
//!               (esp-hal driver gated `not_supported`); bit-bang is
//!               sufficient for averaging-based DAC verification.
//!
//! 1 kHz is low enough that a passive RC at the chip's pin parasitics
//! averages to a clean DC level on the LA's analog input over a
//! 100 us+ window. If the bench has any extra capacitance on these
//! lines, the analog reading will track even more cleanly.
//!
//! ## PASS criteria
//!
//! Firmware-side: all 5 duty stages run + UART log emitted +
//! `=== test_pwm_dac_adc_w_logicpro: PASS (verify on Logic Pro 16) ===`.
//! True PASS requires the host-side Logic Pro 16 capture to confirm:
//!   1. Group A digital edges on CH8 == CH12 (short integrity)
//!   2. Group B digital edges on CH14 == CH15 (short integrity)
//!   3. Analog avg per stage within ±5% of the expected V above

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

/// Pseudo-DAC PWM source pins.
const PWM_A_PIN: u32 = 20;
const PWM_B_PIN: u32 = 48;

/// PWM period -- 1 kHz nominal.
const PWM_PERIOD_US: u64 = 1000;
/// Resolution: 100 substeps per period -> 10 us per step (1% duty).
const PWM_STEP_US: u64 = 10;
const PWM_STEPS: u64 = PWM_PERIOD_US / PWM_STEP_US;

/// Duration to hold each duty stage (seconds).
const STAGE_HOLD_S: u32 = 2;

/// Duty levels to sweep, in percent. 0 and 100 are saturating cases
/// that exercise the "all-low" / "all-high" code paths in the loop.
const DUTY_LEVELS_PCT: &[u32] = &[0, 25, 50, 75, 100];

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    // IO MUX has a 4-byte reserved slot before pin[0] on P4.
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

#[inline(always)]
fn gpio_func_out_sel_reg(pin: u32) -> *mut u32 {
    (GPIO_BASE + 0x558 + pin * 4) as *mut u32
}

/// Configure `pin` as a plain GPIO output (function 1, drv 2).
fn init_pin_output(pin: u32) {
    unsafe {
        // IO MUX: MCU_SEL = function 1 (GPIO), drive strength = 2.
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        r.write_volatile(val);

        // GPIO matrix: route GPIO_OUT_REG bit `pin` to this pin.
        // OUT_SEL = 256 (no signal mux, take from GPIO_OUT_REG bit `pin`)
        // OEN_SEL bit 10 = 1 (output enable from GPIO_ENABLE_REG bit `pin`)
        gpio_func_out_sel_reg(pin).write_volatile(0x100 | (1 << 10));

        // ENABLE_W1TS (set bit) for output enable. Bank 0 = pins 0..31, bank 1 = 32..54.
        let (en_w1ts_off, bit) = if pin < 32 {
            (0x24u32, pin)
        } else {
            (0x30u32, pin - 32)
        };
        ((GPIO_BASE + en_w1ts_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

/// Drive `pin` HIGH or LOW via GPIO_OUT_W1TS / W1TC.
#[inline(always)]
fn pin_set(pin: u32, level_high: bool) {
    unsafe {
        // bank 0 (0..31): W1TS=0x08, W1TC=0x0C
        // bank 1 (32..54): W1TS=0x14, W1TC=0x18
        let (w1ts_off, w1tc_off, bit) = if pin < 32 {
            (0x08u32, 0x0Cu32, pin)
        } else {
            (0x14u32, 0x18u32, pin - 32)
        };
        let off = if level_high { w1ts_off } else { w1tc_off };
        ((GPIO_BASE + off) as *mut u32).write_volatile(1u32 << bit);
    }
}

/// Busy-wait until `target` `Instant` is reached. Uses SYSTIMER under
/// the hood so timing is accurate.
#[inline(always)]
fn busy_until(target: Instant) {
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

/// Run the PWM loop on both PWM pins for `hold_s` seconds at the given
/// duty cycle (in percent, 0..=100). Both pins emit identical waveforms
/// so a single duty value drives both shorted pairs simultaneously.
fn run_pwm(duty_pct: u32, hold_s: u32) {
    let on_us = (PWM_STEPS as u32 * duty_pct / 100) as u64 * PWM_STEP_US;
    let _off_us = PWM_PERIOD_US - on_us;
    let end = Instant::now() + Duration::from_secs(hold_s as u64);

    while Instant::now() < end {
        let cycle_start = Instant::now();
        if duty_pct == 0 {
            // Pure LOW for one period -- no transitions.
            pin_set(PWM_A_PIN, false);
            pin_set(PWM_B_PIN, false);
            busy_until(cycle_start + Duration::from_micros(PWM_PERIOD_US));
        } else if duty_pct == 100 {
            // Pure HIGH for one period -- no transitions.
            pin_set(PWM_A_PIN, true);
            pin_set(PWM_B_PIN, true);
            busy_until(cycle_start + Duration::from_micros(PWM_PERIOD_US));
        } else {
            pin_set(PWM_A_PIN, true);
            pin_set(PWM_B_PIN, true);
            busy_until(cycle_start + Duration::from_micros(on_us));
            pin_set(PWM_A_PIN, false);
            pin_set(PWM_B_PIN, false);
            busy_until(cycle_start + Duration::from_micros(PWM_PERIOD_US));
        }
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_pwm_dac_adc_w_logicpro -- Step 1 of LEDC+SARADC verify");
    info!("===========================================================");
    info!("PWM A: GPIO{} (LA CH12, J1-11) -- shorted to GPIO22 (CH8)", PWM_A_PIN);
    info!("PWM B: GPIO{} (LA CH14, J1-35) -- shorted to GPIO53 (CH15)", PWM_B_PIN);
    info!("PWM freq: 1 kHz  resolution: 1%  hold-per-stage: {}s", STAGE_HOLD_S);
    info!("");
    info!("Logic Pro 16 setup:");
    info!("  digital ch 1, 8, 12, 14, 15 @ 5 MS/s, threshold 1.8 V");
    info!("  analog  ch 8, 12, 14, 15");
    info!("  capture >= 12 s, Async Serial @ CH1 115200 8N1");
    info!("");

    init_pin_output(PWM_A_PIN);
    init_pin_output(PWM_B_PIN);
    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);

    info!("=== test_pwm_dac_adc_w_logicpro: STAGE_BEGIN ===");
    for &duty in DUTY_LEVELS_PCT {
        // Print the marker BEFORE the stage so the LA capture's UART
        // decode time-aligns the duty change to the PWM transition.
        info!("  STAGE duty={}% expected_avg={}.{:02}V",
              duty,
              (duty * 33) / 1000,                    // integer part of 3.30 * duty/100
              ((duty * 33) % 1000) / 10,             // 2-digit fraction
        );
        run_pwm(duty, STAGE_HOLD_S);
    }

    // Park PWM lines LOW so they don't keep toggling during park_alive.
    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_pwm_dac_adc_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_pwm_dac_adc_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_pwm_dac_adc_w_logicpro");
}
