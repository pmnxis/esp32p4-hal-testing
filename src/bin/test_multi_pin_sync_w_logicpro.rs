// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: simultaneous edge across many pins (4-bit Gray-code emit).
//!
//! Cross-pin synchrony depends on:
//!   - both bank-0 W1TS and bank-1 W1TS finishing in the same CPU cycle
//!     window (we know from `test_gpio_bank_boundary` that the
//!     intra-bank atomic write is exact; this bin extends to cross-bank
//!     atomicity), and
//!   - the IO_MUX -> pad propagation being identical across pins.
//!
//! On each step we drive a fresh 4-bit Gray code on (CH9, CH10, CH11,
//! CH13) = (GPIO33, GPIO26, GPIO27, GPIO46). Gray-code wraps cleanly
//! and produces a single-bit-changes-per-step pattern, easy to verify
//! visually on the LA waveform.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   bit 0 -> GPIO33 (LA CH9,  bank 1)
//!   bit 1 -> GPIO26 (LA CH10, bank 0)
//!   bit 2 -> GPIO27 (LA CH11, bank 0)
//!   bit 3 -> GPIO46 (LA CH13, bank 1)
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1, CH9, CH10, CH11, CH13
//!   Sample rate        : 5 MS/s
//!   Threshold          : 1.8 V
//!
//! ## PASS criteria
//!
//! Firmware-side: bin runs all 16 Gray-code states many times, prints PASS.
//!
//! Host-side: at every Gray-code transition, exactly *one* of the four
//! channels changes value, and the change is detectable within < 1 us
//! across all four channels (i.e. cross-bank skew is sub-microsecond).
//! 16-state cycle * NUM_CYCLES iterations -> NUM_CYCLES * 16 transitions
//! per channel total (across the 4 pins, each one toggles once per 4
//! Gray-code positions on average due to single-bit-change property).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const PINS: [u32; 4] = [33, 26, 27, 46];

const STEP_US: u32 = 200;
const NUM_CYCLES: u32 = 256;

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
fn drive_4bit(state: u8) {
    // Compose two atomic masks (bank 0 / bank 1) then write each.
    let mut bank0_set: u32 = 0;
    let mut bank0_clr: u32 = 0;
    let mut bank1_set: u32 = 0;
    let mut bank1_clr: u32 = 0;
    for (i, &pin) in PINS.iter().enumerate() {
        let high = (state >> i) & 1 != 0;
        let bit = if pin < 32 { 1u32 << pin } else { 1u32 << (pin - 32) };
        if pin < 32 {
            if high { bank0_set |= bit } else { bank0_clr |= bit }
        } else {
            if high { bank1_set |= bit } else { bank1_clr |= bit }
        }
    }
    unsafe {
        if bank0_set != 0 {
            ((GPIO_BASE + 0x08) as *mut u32).write_volatile(bank0_set);
        }
        if bank0_clr != 0 {
            ((GPIO_BASE + 0x0C) as *mut u32).write_volatile(bank0_clr);
        }
        if bank1_set != 0 {
            ((GPIO_BASE + 0x14) as *mut u32).write_volatile(bank1_set);
        }
        if bank1_clr != 0 {
            ((GPIO_BASE + 0x18) as *mut u32).write_volatile(bank1_clr);
        }
    }
}

#[inline(always)]
fn gray(n: u8) -> u8 {
    n ^ (n >> 1)
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_multi_pin_sync_w_logicpro -- 4-bit Gray code, 4 channels");
    info!("===========================================================");
    info!("Pins: bit0=GPIO33 bit1=GPIO26 bit2=GPIO27 bit3=GPIO46");
    info!("LA  : bit0=CH9    bit1=CH10   bit2=CH11   bit3=CH13");
    info!("Step: {} us; cycles: {}", STEP_US, NUM_CYCLES);
    info!("");

    for &p in &PINS {
        init_pin_output(p);
    }
    drive_4bit(0);

    info!("=== test_multi_pin_sync_w_logicpro: STAGE_BEGIN ===");
    for cycle in 0..NUM_CYCLES {
        for n in 0..16u8 {
            drive_4bit(gray(n) & 0xF);
            esp32p4_hal_testing::busy_delay(STEP_US * 200);
        }
        if cycle % 32 == 0 {
            info!("  cycle {} / {}", cycle, NUM_CYCLES);
        }
    }
    drive_4bit(0);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_multi_pin_sync_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_multi_pin_sync_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_multi_pin_sync_w_logicpro");
}
