// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: PWM frequency sweep, verified at edge level via Logic Pro 16.
//!
//! Companion to `test_pwm_dac_adc_w_logicpro` (which sweeps duty at fixed
//! 1 kHz). This bin holds duty at 50 % and sweeps PWM frequency across
//! 1 kHz / 10 kHz / 100 kHz / 1 MHz, 2 s per stage. Useful to confirm
//! the host-side LA can decode PWM at the rates the LEDC peripheral
//! would target once we wire that driver up.
//!
//! Bit-banged via direct GPIO MMIO (LEDC driver not yet on P4).
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO20 (PWM A)  ─── short ─── GPIO22 (ADC1_CH6)
//!         LA CH12  /  J1-11    ─── LA CH8   /  J1-12
//!   GPIO48 (PWM B)  ─── short ─── GPIO53 (ADC2)
//!         LA CH14  /  J1-35    ─── LA CH15  /  J1-37
//!
//! Only the PWM pin is configured as output. ADC pins remain input-only.
//!
//! ## Logic Pro 16 setup
//!
//!   Digital channels enabled : CH1, CH8, CH12, CH14, CH15
//!   Digital sample rate      : >= 25 MS/s   (1 MHz PWM needs >= 10x)
//!   Digital threshold        : 1.8 V
//!   Capture duration         : >= 10 s   (4 stages * 2 s + margin)
//!   Async Serial analyzer    : CH1 @ 115200 8N1 (verdict text)
//!
//!   Per-stage expected edge rate (rises in 2 s):
//!       1 kHz   ->     2_000 rises
//!      10 kHz   ->    20_000 rises
//!     100 kHz   ->   200_000 rises
//!       1 MHz   -> 2_000_000 rises (CPU-bound; actual rate may droop)
//!
//! At 1 MHz the CPU spin-loop / volatile-write overhead dominates.
//! The exact achieved rate depends on cache / pipeline state -- expect
//! the LA to report 800 kHz - 1.0 MHz, not exactly 1 MHz. 1 kHz / 10 kHz
//! / 100 kHz stages should be within 1 % of nominal.
//!
//! ## PASS criteria
//!
//! Firmware-side: all 4 stages run + UART log emitted +
//! `=== test_pwm_freq_sweep_w_logicpro: PASS (verify on Logic Pro 16) ===`.
//! True PASS requires the host-side capture to confirm:
//!   1. Each stage's edge count on CH12 / CH14 within ±5% of expected
//!      (1 MHz stage: ±20 % allowed for CPU-loop droop)
//!   2. CH8 == CH12 and CH15 == CH14 transitions match (short integrity)

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const PWM_A_PIN: u32 = 20;
const PWM_B_PIN: u32 = 48;

const STAGE_HOLD_S: u32 = 2;

/// (label_hz, half_period_us) -- 50% duty so HIGH and LOW are both
/// `half_period_us`. Stages are deliberately picked at orders of
/// magnitude so the LA can decode each cleanly.
const STAGES: &[(u32, u64)] = &[
    (    1_000,  500), //   1 kHz
    (   10_000,   50), //  10 kHz
    (  100_000,    5), // 100 kHz   -- 5 us half-period
    (1_000_000,    1), //   1 MHz   -- 1 us half-period (CPU-bound)
];

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

        let (en_w1ts_off, bit) = if pin < 32 {
            (0x24u32, pin)
        } else {
            (0x30u32, pin - 32)
        };
        ((GPIO_BASE + en_w1ts_off) as *mut u32).write_volatile(1u32 << bit);
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

/// Toggle both pins at a fixed 50% duty for `hold_s` seconds.
/// `half_period_us` >= 1; for the 1 MHz stage we busy-loop without a
/// SYSTIMER check on each half-cycle (the SYSTIMER overhead alone
/// exceeds 1 us at 400 MHz CPU; we'd never get a transition through).
fn run_pwm(half_period_us: u64, hold_s: u32) {
    let end = Instant::now() + Duration::from_secs(hold_s as u64);

    if half_period_us >= 5 {
        // Slow path: SYSTIMER-bounded each half-cycle (accurate).
        while Instant::now() < end {
            let t0 = Instant::now();
            pin_set(PWM_A_PIN, true);
            pin_set(PWM_B_PIN, true);
            while Instant::now() - t0 < Duration::from_micros(half_period_us) {
                core::hint::spin_loop();
            }
            let t1 = Instant::now();
            pin_set(PWM_A_PIN, false);
            pin_set(PWM_B_PIN, false);
            while Instant::now() - t1 < Duration::from_micros(half_period_us) {
                core::hint::spin_loop();
            }
        }
    } else {
        // Fast path: tight loop without per-edge SYSTIMER read. Edge
        // rate is set by `nop_count` chosen so 2 * (toggle + nop) ≈
        // 2 * half_period_us. At 400 MHz CPU and 1 us half-period
        // we want ~400 cycles per half-cycle. Empirical: each iter
        // costs ~3 cycles for write_volatile + spin_loop, so use 130.
        let nop_count = 130u32;
        while Instant::now() < end {
            pin_set(PWM_A_PIN, true);
            pin_set(PWM_B_PIN, true);
            for _ in 0..nop_count {
                core::hint::spin_loop();
            }
            pin_set(PWM_A_PIN, false);
            pin_set(PWM_B_PIN, false);
            for _ in 0..nop_count {
                core::hint::spin_loop();
            }
        }
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_pwm_freq_sweep_w_logicpro -- PWM 1k/10k/100k/1M @50% duty");
    info!("===========================================================");
    info!("PWM A: GPIO{} (LA CH12, J1-11)", PWM_A_PIN);
    info!("PWM B: GPIO{} (LA CH14, J1-35)", PWM_B_PIN);
    info!("Duty fixed at 50%; sweep frequency.");
    info!("Stage hold: {}s.  Total run time ~ {}s.", STAGE_HOLD_S, STAGE_HOLD_S * STAGES.len() as u32);
    info!("");
    info!("Logic Pro 16 setup:");
    info!("  digital ch 1, 8, 12, 14, 15 @ >= 25 MS/s, threshold 1.8 V");
    info!("  capture >= 10 s, Async Serial @ CH1 115200 8N1");
    info!("");

    init_pin_output(PWM_A_PIN);
    init_pin_output(PWM_B_PIN);
    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);

    info!("=== test_pwm_freq_sweep_w_logicpro: STAGE_BEGIN ===");
    for &(freq_hz, half_us) in STAGES {
        info!(
            "  STAGE freq={}Hz half_period={}us expected_rises={}",
            freq_hz,
            half_us,
            freq_hz * STAGE_HOLD_S,
        );
        run_pwm(half_us, STAGE_HOLD_S);
    }

    pin_set(PWM_A_PIN, false);
    pin_set(PWM_B_PIN, false);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_pwm_freq_sweep_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_pwm_freq_sweep_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_pwm_freq_sweep_w_logicpro");
}
