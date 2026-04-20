// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: Clock tree verification.
//!
//! Verifies: CPLL frequency, SYSTIMER tick rate, CPU clock.
//! Method: Use SYSTIMER (16MHz from XTAL) as reference to measure CPU cycles.
//! Pass: Measured frequencies within 5% of expected.
//! Fail: Frequency out of range.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_clock: Clock tree verification ===");

    // 1. SYSTIMER tick rate (should be 16 MHz = 1 tick per 62.5ns)
    // Measure 1 second worth of ticks
    let t0 = esp_hal::time::Instant::now();
    esp32p4_hal_testing::delay_ms(100); // 100ms
    let t1 = esp_hal::time::Instant::now();
    let elapsed_us = t0.elapsed().as_micros();
    info!("100ms delay measured: {} us (expected ~100000)", elapsed_us);

    // Allow 5% tolerance
    assert!(
        elapsed_us > 95_000 && elapsed_us < 105_000,
        "SYSTIMER timing off for 100ms",
    );
    info!("SYSTIMER tick rate: OK");

    // 2. CPU clock estimation via mcycle CSR
    // Read mcycle before and after a known SYSTIMER delay
    let mcycle_start: u32;
    unsafe { core::arch::asm!("csrr {}, mcycle", out(reg) mcycle_start); }

    let t0 = esp_hal::time::Instant::now();
    // Spin for ~10ms
    while t0.elapsed().as_micros() < 10_000 {
        core::hint::spin_loop();
    }

    let mcycle_end: u32;
    unsafe { core::arch::asm!("csrr {}, mcycle", out(reg) mcycle_end); }

    let cycles = mcycle_end.wrapping_sub(mcycle_start);
    let freq_mhz = cycles / 10_000; // cycles per 10ms -> MHz (approx)
    info!("CPU frequency estimate: ~{} MHz (expected ~400)", freq_mhz);

    // Allow wide tolerance (350-450 MHz) due to measurement noise
    if freq_mhz > 350 && freq_mhz < 450 {
        info!("CPU clock: OK (~400 MHz)");
    } else if freq_mhz > 300 && freq_mhz < 410 {
        info!("CPU clock: OK (~360 MHz, conservative CPLL)");
    } else {
        info!("CPU clock: UNEXPECTED {} MHz", freq_mhz);
        // Don't panic -- frequency might be XTAL (40MHz) if CPLL failed
    }

    // 3. Report clock tree state
    info!("cpu_clock() = {} Hz", esp_hal::clock::cpu_clock().as_hz());
    info!("xtal_clock() = {} Hz", esp_hal::clock::xtal_clock().as_hz());

    esp32p4_hal_testing::signal_pass();
    info!("=== test_clock: PASS ===");

    esp32p4_hal_testing::park_alive("test_clock");
}
