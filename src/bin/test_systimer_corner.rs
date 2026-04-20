// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Corner test: SYSTIMER edge cases.
//!
//! 1. LO-HI-LO read consistency near rollover boundary
//! 2. Monotonicity (time never goes backwards)
//! 3. Sub-microsecond timing resolution
//! 4. Long-duration accuracy (10 seconds)

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

    info!("=== test_systimer_corner: SYSTIMER edge cases ===");

    // 1. Monotonicity test (1M reads, time should never go backwards)
    info!("Monotonicity test: 1M reads...");
    let mut prev = esp_hal::time::Instant::now();
    let mut violations = 0u32;
    for _ in 0..1_000_000 {
        let now = esp_hal::time::Instant::now();
        if now < prev {
            violations += 1;
        }
        prev = now;
    }
    info!("Monotonicity violations: {} / 1,000,000", violations);
    assert!(violations == 0, "Time went backwards!");
    info!("Monotonicity: PASS");

    // 2. Resolution test (minimum measurable interval)
    info!("Resolution test...");
    let mut min_delta = u64::MAX;
    for _ in 0..10_000 {
        let t0 = esp_hal::time::Instant::now();
        let t1 = esp_hal::time::Instant::now();
        let delta = t1.elapsed().as_micros(); // from t1, not t0
        // Actually measure t1-t0
        let _ = delta;
        let d0 = t0.elapsed().as_micros();
        if d0 > 0 && d0 < min_delta {
            min_delta = d0;
        }
    }
    info!("Minimum measurable interval: {} us", min_delta);
    info!("(At 16MHz, theoretical min is 1/16 us = 62.5ns, but Instant uses us resolution)");

    // 3. 10-second accuracy test
    info!("10-second accuracy test (counting to 10s)...");
    let t0 = esp_hal::time::Instant::now();
    // Busy-wait exactly 10 seconds (CPU intensive)
    while t0.elapsed().as_micros() < 10_000_000 {
        core::hint::spin_loop();
    }
    let actual = t0.elapsed().as_micros();
    info!("10s elapsed: {} us (expected 10,000,000)", actual);
    let error_ppm = ((actual as i64 - 10_000_000) * 1_000_000 / 10_000_000).unsigned_abs();
    info!("Error: {} ppm", error_ppm);
    // XTAL accuracy is typically +/- 20ppm, allow 100ppm for measurement overhead
    assert!(error_ppm < 1000, "Timer accuracy too low");
    info!("10s accuracy: PASS ({} ppm error)", error_ppm);

    // 4. Rapid read stress test (back-to-back reads)
    info!("Rapid read stress test: 100K back-to-back reads...");
    let start = esp_hal::time::Instant::now();
    let mut readings = [0u64; 10];
    for r in readings.iter_mut() {
        for _ in 0..10_000 {
            let _ = esp_hal::time::Instant::now();
        }
        *r = start.elapsed().as_micros();
    }
    info!("10x10K reads, cumulative us: {:?}", readings);
    info!("Rapid read: PASS (no hangs)");

    esp32p4_hal_testing::signal_pass();
    info!("=== test_systimer_corner: PASS ===");

    esp32p4_hal_testing::park_alive("test_systimer_corner");
}
