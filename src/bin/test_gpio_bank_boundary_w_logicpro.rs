// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: GPIO bank 0 / bank 1 boundary toggling, verified at edge level
//! by Logic Pro 16.
//!
//! P4 has 55 GPIOs split across two banks: bank 0 (GPIO0..31) and bank 1
//! (GPIO32..54). The OUT / OUT_W1TS / OUT_W1TC / ENABLE registers all
//! switch offsets at the boundary:
//!
//!   bank 0 OUT / W1TS / W1TC : 0x04 / 0x08 / 0x0C
//!   bank 1 OUT / W1TS / W1TC : 0x10 / 0x14 / 0x18
//!   bank 0 ENABLE / W1TS / W1TC : 0x20 / 0x24 / 0x28
//!   bank 1 ENABLE / W1TS / W1TC : 0x2C / 0x30 / 0x34
//!
//! Wrong bit indexing (using `gpio_num` directly instead of
//! `gpio_num - 32`) is a common P4 esp-hal bug that silently drops
//! writes to bank 1 pins. This bin proves the boundary handling by
//! toggling pins on either side of it in lockstep:
//!
//!   GPIO33 (CH9 / J1-31)   bank 1 first
//!   GPIO26 (CH10 / J1-33)  bank 0 last available
//!   GPIO27 (CH11 / J1-38)  bank 0 last available
//!
//! The bin emits a 5-stage edge-rate sweep (1 / 10 / 100 / 1000 kHz)
//! on all three pins simultaneously. The LA verifies all three
//! channels show identical edge counts at the same instants.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO33 - LA CH9  - J1 pin 31  (bank 1, bit 1 in IN1 / OUT1)
//!   GPIO26 - LA CH10 - J1 pin 33  (bank 0, bit 26 in IN / OUT)
//!   GPIO27 - LA CH11 - J1 pin 38  (bank 0, bit 27)
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1 (verdict), CH9, CH10, CH11
//!   Digital sample rate: 25 MS/s   (1 MHz top stage)
//!   Digital threshold  : 1.8 V
//!   Capture duration   : >= 12 s
//!   Async Serial @ CH1 : 115200 8N1
//!
//! ## PASS criteria
//!
//! Firmware-side: all stages run, verdict line emitted.
//!
//! Host-side: per stage, edge counts on CH9 / CH10 / CH11 match the
//! expected count within ±5 % (±20 % at 1 MHz for CPU-loop droop).
//! Timing of edges across the three channels must be co-located
//! within < 1 us (they're written in the same write_volatile loop;
//! relative skew should be a few CPU cycles only).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

// Pins to toggle. Pick one bank-1 pin and two bank-0 pins so any per-
// bank bug shows up as a divergence.
const PINS: &[u32] = &[33, 26, 27];

const STAGE_HOLD_S: u32 = 2;

const STAGES: &[(u32, u64)] = &[
    (    1_000, 500),  // 1 kHz, 500 us half period
    (   10_000,  50),
    (  100_000,   5),
    (1_000_000,   1),
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

/// Drive multiple pins atomically using a single W1TS / W1TC write per
/// bank. This eliminates per-pin timing skew at the silicon level.
#[inline(always)]
fn pins_set_all(pins: &[u32], level_high: bool) {
    let mut bank0_mask: u32 = 0;
    let mut bank1_mask: u32 = 0;
    for &pin in pins {
        if pin < 32 {
            bank0_mask |= 1 << pin;
        } else {
            bank1_mask |= 1 << (pin - 32);
        }
    }
    unsafe {
        if bank0_mask != 0 {
            let off = if level_high { 0x08u32 } else { 0x0Cu32 };
            ((GPIO_BASE + off) as *mut u32).write_volatile(bank0_mask);
        }
        if bank1_mask != 0 {
            let off = if level_high { 0x14u32 } else { 0x18u32 };
            ((GPIO_BASE + off) as *mut u32).write_volatile(bank1_mask);
        }
    }
}

#[inline(always)]
fn busy_until(target: Instant) {
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

fn run_pwm(half_us: u64, hold_s: u32) {
    let end = Instant::now() + Duration::from_secs(hold_s as u64);
    if half_us >= 5 {
        while Instant::now() < end {
            let t0 = Instant::now();
            pins_set_all(PINS, true);
            while Instant::now() - t0 < Duration::from_micros(half_us) {
                core::hint::spin_loop();
            }
            let t1 = Instant::now();
            pins_set_all(PINS, false);
            while Instant::now() - t1 < Duration::from_micros(half_us) {
                core::hint::spin_loop();
            }
        }
    } else {
        // Tight loop, 1 us half-period (~ 1 MHz)
        let nop_count = 130u32;
        while Instant::now() < end {
            pins_set_all(PINS, true);
            for _ in 0..nop_count { core::hint::spin_loop(); }
            pins_set_all(PINS, false);
            for _ in 0..nop_count { core::hint::spin_loop(); }
        }
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_gpio_bank_boundary_w_logicpro -- bank 0/1 atomic toggle");
    info!("===========================================================");
    info!("Pins: GPIO{} (bank 1, CH9), GPIO{} (bank 0, CH10), GPIO{} (bank 0, CH11)",
          PINS[0], PINS[1], PINS[2]);
    info!("Pattern: 50%% duty, freq 1k/10k/100k/1M Hz, {}s each", STAGE_HOLD_S);
    info!("Atomic: single W1TS/W1TC per bank per edge.");
    info!("");
    info!("Logic Pro 16: digital CH1+9+10+11 @ 25 MS/s, threshold 1.8 V");
    info!("");

    for &p in PINS {
        init_pin_output(p);
    }
    pins_set_all(PINS, false);

    info!("=== test_gpio_bank_boundary_w_logicpro: STAGE_BEGIN ===");
    for &(freq_hz, half_us) in STAGES {
        info!("  STAGE freq={}Hz half={}us expected_rises={}",
              freq_hz, half_us, freq_hz * STAGE_HOLD_S);
        run_pwm(half_us, STAGE_HOLD_S);
    }
    pins_set_all(PINS, false);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_gpio_bank_boundary_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_gpio_bank_boundary_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_gpio_bank_boundary_w_logicpro");
}
