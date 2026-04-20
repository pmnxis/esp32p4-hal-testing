// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! ESP32-P4 HAL Testing -- common utilities.
//!
//! Each test is a separate binary in src/bin/.
//! Logging via esp-println (UART0 on GPIO37 TX / GPIO38 RX).
//! Panic handler via esp-backtrace (prints backtrace + halts).
//!
//! Build:  cargo +nightly build --bin test_init
//! Flash:  cargo +nightly run --bin test_init
//!
//! Tests that pass blink LED rapidly (GPIO23).
//! Tests that fail hold LED steady on and print error.
//!
//! Future: subset of these will be ported to esp-hal qa-test with
//! `//% CHIPS: esp32p4` metadata for upstream CI.

#![no_std]

/// GPIO23 LED on EV Board.
const LED_PIN: u32 = 23;
const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

/// Initialize LED GPIO23 as output.
pub fn init_led() {
    unsafe {
        // IO MUX: function 1 (GPIO), drive strength 2.
        // Per esp32p4 PAC, IO_MUX has a 4-byte reserved slot before pin[0], so
        // pin N is at BASE + 0x04 + N*4 (NOT BASE + N*4 as on other chips).
        let iomux = (IO_MUX_BASE + 0x04 + LED_PIN * 4) as *mut u32;
        let val = iomux.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12);
        let val = (val & !(0x3 << 10)) | (2 << 10);
        iomux.write_volatile(val);

        // GPIO_FUNC_OUT_SEL: OUT_SEL=256 (bit 8 set -> use GPIO_OUT_REG),
        // OEN_SEL=1 (bit 10 -> take output-enable from GPIO_ENABLE_REG)
        ((GPIO_BASE + 0x558 + LED_PIN * 4) as *mut u32).write_volatile(0x100 | (1 << 10));

        // Enable output: ENABLE_W1TS (per esp32p4 PAC) is 0x24, NOT 0x28 (0x28 is ENABLE_W1TC).
        ((GPIO_BASE + 0x24) as *mut u32).write_volatile(1 << LED_PIN);
    }
}

/// Signal test PASS: rapid LED blink (10 times).
pub fn signal_pass() {
    init_led();
    let mask = 1u32 << LED_PIN;
    for _ in 0..10 {
        unsafe {
            ((GPIO_BASE + 0x08) as *mut u32).write_volatile(mask);
        }
        busy_delay(200_000);
        unsafe {
            ((GPIO_BASE + 0x0C) as *mut u32).write_volatile(mask);
        }
        busy_delay(200_000);
    }
}

/// Signal test FAIL: LED steady on, halt.
pub fn signal_fail() -> ! {
    init_led();
    unsafe {
        ((GPIO_BASE + 0x08) as *mut u32).write_volatile(1u32 << LED_PIN);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Busy-wait delay (approximate).
pub fn busy_delay(cycles: u32) {
    for _ in 0..cycles {
        core::hint::spin_loop();
    }
}

/// Delay using SYSTIMER (accurate).
pub fn delay_ms(ms: u32) {
    let start = esp_hal::time::Instant::now();
    let target = esp_hal::time::Duration::from_millis(ms as u64);
    while start.elapsed() < target {
        core::hint::spin_loop();
    }
}

/// Terminal-state heartbeat: prints `[alive] <tag> #<n>` every second forever.
/// Tests should call this at the end of `main` so any host that attaches
/// late still sees evidence the test completed. Replaces a silent
/// `loop { delay_ms(1000); }`.
pub fn park_alive(tag: &str) -> ! {
    let mut n: u32 = 0;
    loop {
        log::info!("[alive] {} #{}", tag, n);
        n = n.wrapping_add(1);
        delay_ms(1000);
    }
}
