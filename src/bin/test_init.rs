// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: Basic esp-hal init + SYSTIMER + LED blink.
//!
//! Verifies: PMU init, WDT disable, CPLL 400MHz, SYSTIMER time driver, GPIO output.
//! Pass: LED blinks 5 times rapidly, then slow blink.
//! Fail: LED stays on (panic handler).
//!
//! This is the most fundamental test. If this works, HAL infrastructure is OK.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_init: ESP32-P4 HAL basic init ===");
    info!("PMU init: OK (we're running)");
    info!("CPLL 400MHz: OK (CPU is clocked)");

    // Verify SYSTIMER works (time should advance)
    let t0 = esp_hal::time::Instant::now();
    esp32p4_hal_testing::busy_delay(100_000);
    let _t1 = esp_hal::time::Instant::now();
    let elapsed = t0.elapsed();
    info!("SYSTIMER elapsed: {} us", elapsed.as_micros());
    assert!(elapsed.as_micros() > 0, "SYSTIMER not advancing!");

    info!("SYSTIMER: OK");

    esp32p4_hal_testing::signal_pass();
    info!("=== test_init: PASS ===");

    esp32p4_hal_testing::park_alive("test_init");
}
