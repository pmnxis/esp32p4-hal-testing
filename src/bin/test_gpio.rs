// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: GPIO output and readback.
//!
//! Verifies: GPIO output drive, GPIO input read, IO MUX configuration.
//! Method: Configure GPIO23 as output, read back via GPIO_IN register.
//! Can also test with two GPIOs shorted together (e.g. GPIO20 -> GPIO21).
//!
//! Pass: Output state matches readback.
//! Fail: Mismatch.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;
// Offsets per esp32p4 PAC gpio::RegisterBlock.
// NOTE: previous offsets (0x28 enable_w1ts, 0x4C gpio_in) were for ESP32-
// class chips; on P4 the layout is different (out/out1, enable/enable1,
// in/in1 pairs for GPIO0-31 / GPIO32-54).
const GPIO_OUT_W1TS: u32 = GPIO_BASE + 0x08;
const GPIO_OUT_W1TC: u32 = GPIO_BASE + 0x0C;
const GPIO_ENABLE_W1TS: u32 = GPIO_BASE + 0x24;
const GPIO_IN: u32 = GPIO_BASE + 0x3C; // GPIO0-31 input state

fn gpio_init_output(pin: u32) {
    unsafe {
        let iomux = (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32;
        let val = iomux.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO)
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        iomux.write_volatile(val);
        // func_out_sel_cfg: OUT_SEL=256 (0x100, bits 0:8) -> drive from GPIO_OUT_REG,
        // OEN_SEL=1 (bit 10) -> take output-enable from GPIO_ENABLE_REG (not from a
        // peripheral's OE signal which is 0/disabled).
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        (GPIO_ENABLE_W1TS as *mut u32).write_volatile(1 << pin);
    }
}

fn gpio_init_input(pin: u32) {
    unsafe {
        let iomux = (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32;
        let val = iomux.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO)
        let val = val | (1 << 9); // FUN_IE = 1 (input enable)
        iomux.write_volatile(val);
    }
}

fn gpio_set(pin: u32, high: bool) {
    unsafe {
        if high {
            (GPIO_OUT_W1TS as *mut u32).write_volatile(1 << pin);
        } else {
            (GPIO_OUT_W1TC as *mut u32).write_volatile(1 << pin);
        }
    }
}

fn gpio_read(pin: u32) -> bool {
    unsafe { (GPIO_IN as *const u32).read_volatile() & (1 << pin) != 0 }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_gpio: GPIO output/input ===");

    // Test 1: Output + readback on same pin (GPIO23 = LED)
    // When GPIO is output, reading GPIO_IN should reflect output state
    let test_pin: u32 = 23;
    gpio_init_output(test_pin);
    gpio_init_input(test_pin); // also enable input on same pin

    gpio_set(test_pin, true);
    esp32p4_hal_testing::busy_delay(1000);
    let high = gpio_read(test_pin);
    info!("GPIO{} set HIGH, read: {}", test_pin, high);
    assert!(high, "GPIO readback should be HIGH");

    gpio_set(test_pin, false);
    esp32p4_hal_testing::busy_delay(1000);
    let low = gpio_read(test_pin);
    info!("GPIO{} set LOW, read: {}", test_pin, low);
    assert!(!low, "GPIO readback should be LOW");

    info!("GPIO output/readback: OK");

    // Test 2: GPIO toggle speed (measure with SYSTIMER)
    let t0 = esp_hal::time::Instant::now();
    for _ in 0..10_000 {
        gpio_set(test_pin, true);
        gpio_set(test_pin, false);
    }
    let elapsed = t0.elapsed();
    info!(
        "10K GPIO toggles: {} us ({} ns/toggle)",
        elapsed.as_micros(),
        elapsed.as_micros() * 1000 / 10_000
    );

    esp32p4_hal_testing::signal_pass();
    info!("=== test_gpio: PASS ===");

    esp32p4_hal_testing::park_alive("test_gpio");
}
