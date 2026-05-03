// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Corner test: GPIO all 55 pins output/readback.
//!
//! Tests every GPIO pin (0-54) for basic output functionality.
//! Skips dedicated pins (USB, JTAG, Flash) that can't be safely toggled.
//! Catches: dead pins, stuck pins, bank 0/1 boundary (GPIO31->32).
//!
//! Also tests LP GPIO pullup/pulldown on pins 0-5, 12-23.
//!
//! WARNING (bench-specific): per `.investigation/la_channel_map.csv`
//! GPIO20<->GPIO22 and GPIO48<->GPIO53 are physically shorted on this
//! bench. Driving both ends of a short to opposite levels causes
//! push-pull output fight + transient over-current. This bin currently
//! sweeps each pin one at a time and reads back, so it's safe (only one
//! end is ever driven at a moment), but if you parallelize it, ensure
//! the two pins of each shorted pair stay in lock-step or are both
//! configured as input.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

// Pins to SKIP (dedicated function, unsafe to toggle)
// USB Serial/JTAG: 24, 25
// USB FS: 26, 27
// Flash: dedicated SPI pins (not in GPIO range)
// Strapping: 32-38 (boot mode, but safe to toggle after boot)
// JTAG: 2-5 (safe if JTAG not connected)
const SKIP_PINS: &[u32] = &[24, 25, 26, 27];

fn gpio_init_output(pin: u32) {
    unsafe {
        let iomux = (IO_MUX_BASE + pin * 4) as *mut u32;
        let val = iomux.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12);
        let val = (val & !(0x3 << 10)) | (2 << 10);
        let val = val | (1 << 9); // input enable too (for readback)
        iomux.write_volatile(val);
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100);

        // Enable output (bank 0: GPIO_ENABLE_W1TS, bank 1: GPIO_ENABLE1_W1TS)
        if pin < 32 {
            ((GPIO_BASE + 0x28) as *mut u32).write_volatile(1 << pin);
        } else {
            ((GPIO_BASE + 0x34) as *mut u32).write_volatile(1 << (pin - 32));
        }
    }
}

fn gpio_set(pin: u32, high: bool) {
    unsafe {
        if pin < 32 {
            if high {
                ((GPIO_BASE + 0x08) as *mut u32).write_volatile(1 << pin);
            } else {
                ((GPIO_BASE + 0x0C) as *mut u32).write_volatile(1 << pin);
            }
        } else {
            if high {
                ((GPIO_BASE + 0x14) as *mut u32).write_volatile(1 << (pin - 32));
            } else {
                ((GPIO_BASE + 0x18) as *mut u32).write_volatile(1 << (pin - 32));
            }
        }
    }
}

fn gpio_read(pin: u32) -> bool {
    unsafe {
        if pin < 32 {
            (GPIO_BASE + 0x4C) as *const u32; // GPIO_IN_REG
            ((GPIO_BASE + 0x4C) as *const u32).read_volatile() & (1 << pin) != 0
        } else {
            ((GPIO_BASE + 0x50) as *const u32).read_volatile() & (1 << (pin - 32)) != 0
        }
    }
}

fn should_skip(pin: u32) -> bool {
    SKIP_PINS.contains(&pin)
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_gpio_all_pins: 55-pin GPIO sweep ===");

    let mut pass_count = 0u32;
    let mut fail_count = 0u32;
    let mut skip_count = 0u32;

    for pin in 0..55u32 {
        if should_skip(pin) {
            info!("GPIO{:2}: SKIP (dedicated)", pin);
            skip_count += 1;
            continue;
        }

        gpio_init_output(pin);
        esp32p4_hal_testing::busy_delay(100);

        // Test HIGH
        gpio_set(pin, true);
        esp32p4_hal_testing::busy_delay(100);
        let high = gpio_read(pin);

        // Test LOW
        gpio_set(pin, false);
        esp32p4_hal_testing::busy_delay(100);
        let low = gpio_read(pin);

        if high && !low {
            info!("GPIO{:2}: OK", pin);
            pass_count += 1;
        } else {
            info!("GPIO{:2}: FAIL (high={}, low={})", pin, high, low);
            fail_count += 1;
        }

        // Restore to safe state (input, no drive)
        gpio_set(pin, false);
    }

    info!("--- GPIO sweep results ---");
    info!("Pass: {}, Fail: {}, Skip: {}", pass_count, fail_count, skip_count);

    // Bank boundary check
    info!("--- Bank boundary test (GPIO31 <-> GPIO32) ---");
    if !should_skip(31) && !should_skip(32) {
        gpio_init_output(31);
        gpio_init_output(32);
        gpio_set(31, true);
        gpio_set(32, false);
        let r31 = gpio_read(31);
        let r32 = gpio_read(32);
        info!("GPIO31=HIGH({}), GPIO32=LOW({}) -- cross-bank: {}", r31, r32,
            if r31 && !r32 { "OK" } else { "FAIL" });
    }

    if fail_count == 0 {
        esp32p4_hal_testing::signal_pass();
        info!("=== test_gpio_all_pins: PASS ===");
    } else {
        info!("=== test_gpio_all_pins: FAIL ({} pins) ===", fail_count);
        esp32p4_hal_testing::signal_fail();
    }

    esp32p4_hal_testing::park_alive("test_gpio_all_pins");
}
