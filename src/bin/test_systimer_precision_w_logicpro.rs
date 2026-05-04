// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: SYSTIMER tick precision, measured via LA edge timing.
//!
//! ESP32-P4 SYSTIMER runs at 16 MHz (XTAL / 2.5; 40 MHz XTAL * 0.4).
//! `esp_hal::time::Instant::now()` returns a SYSTIMER-derived value.
//! `busy_until(target)` is therefore as precise as the SYSTIMER ticks.
//!
//! This bin emits a square wave on GPIO46 (LA CH13) using
//! `busy_until` for each half-period. The Logic Pro captures all
//! edges; an offline script computes the *standard deviation* of
//! observed half-periods. A working SYSTIMER + clock tree should
//! give:
//!   nominal half-period:  PERIOD_US / 2
//!   measured  std-dev:    < 1 us  (limited by SYSTIMER 62.5 ns granularity
//!                                  + tiny CPU dispatch jitter)
//!
//! Multiple frequency stages exercise the same path at different
//! tick counts.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO46 -> LA CH13 -> J1 pin 36
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1, CH13
//!   Sample rate        : 25 MS/s   (40 ns resolution -> measure us-level jitter)
//!   Threshold          : 1.8 V
//!   Capture            : >= 12 s
//!
//! ## PASS criteria
//!
//! Firmware-side: bin runs all stages, prints PASS marker.
//!
//! Host-side: per stage, the standard deviation of observed
//! half-periods is < 5 % of nominal. This is loose -- a healthy
//! SYSTIMER + CPU at 400 MHz should get < 0.5 % easily.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const PIN: u32 = 46;

/// (label, half-period microseconds, hold seconds)
const STAGES: &[(u32, u64, u32)] = &[
    (1_000,  500, 2),  // 1 kHz
    (10_000,  50, 2),  // 10 kHz
    (100_000,  5, 2),  // 100 kHz
];

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

fn init_pin_output(pin: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12);
        let val = (val & !(0x3 << 10)) | (2 << 10);
        r.write_volatile(val);
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        let (en_w1ts_off, bit) = if pin < 32 { (0x24u32, pin) } else { (0x30u32, pin - 32) };
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

#[inline(always)]
fn busy_until(target: Instant) {
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

fn run_systimer_squarewave(half_us: u64, hold_s: u32) {
    let total_us = (hold_s as u64) * 1_000_000;
    let n_edges = total_us / half_us;
    let t0 = Instant::now();
    for i in 0..n_edges {
        let high = i % 2 == 0;
        pin_set(PIN, high);
        let target = t0 + Duration::from_micros((i + 1) * half_us);
        busy_until(target);
    }
    pin_set(PIN, false);
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_systimer_precision_w_logicpro -- SYSTIMER edge timing");
    info!("===========================================================");
    info!("Pin: GPIO{} -> LA CH13 (J1-36)", PIN);
    info!("Method: busy_until(t0 + N*half_us) per edge.  16 MHz SYSTIMER");
    info!("");
    info!("Logic Pro 16: digital CH1+13 @ 25 MS/s, threshold 1.8 V");
    info!("");

    init_pin_output(PIN);
    pin_set(PIN, false);

    info!("=== test_systimer_precision_w_logicpro: STAGE_BEGIN ===");
    for &(label, half_us, hold) in STAGES {
        info!("  STAGE freq={}Hz half={}us hold={}s", label, half_us, hold);
        run_systimer_squarewave(half_us, hold);
    }
    pin_set(PIN, false);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_systimer_precision_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_systimer_precision_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_systimer_precision_w_logicpro");
}
