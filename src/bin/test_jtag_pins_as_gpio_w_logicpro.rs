// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: GPIO4 / GPIO5 used as plain GPIO outputs, verifying that
//! their default JTAG (MTMS / MTDO) function does *not* override the
//! GPIO matrix while MCU_SEL = 1 on this v3.2 silicon.
//!
//! la_channel_map.csv calls these "JTAG MTMS / GPIO" and "JTAG MTDO /
//! GPIO" with the note "no JTAG override observed when MCU_SEL=1".
//! This bin makes that observation reproducible: emit a recognizable
//! pattern (10 pulses with a long inter-cycle gap) on each pin and
//! let the LA confirm it appears on the expected channel.
//!
//! Note: when an actual JTAG cable is plugged in, the chip's bondout
//! arrangement may pull these pins -- in that case this bin will fail
//! and the bench needs to disconnect JTAG before running.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO4 (JTAG MTMS / GPIO)  -> LA CH6  -> J1 pin 20
//!   GPIO5 (JTAG MTDO / GPIO)  -> LA CH7  -> J1 pin 18
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1 (verdict), CH6, CH7
//!   Digital sample rate: 2 MS/s
//!   Threshold          : 1.8 V
//!   Capture            : >= 6 s
//!
//! ## PASS criteria
//!
//! Firmware-side: bin runs, pin pulses emitted, PASS marker.
//!
//! Host-side: CH6 sees ~ 4 (GPIO4) pulses per cycle, CH7 sees ~ 5
//! (GPIO5). Both pin counts match the GPIO number convention used in
//! `test_pin_mapper`. Across 3 cycles, total pulses on:
//!   CH6 == 12 ± 1
//!   CH7 == 15 ± 1
//! confirming GPIO matrix routing wins over the JTAG default.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const PIN_A: u32 = 4;
const PIN_B: u32 = 5;

const CYCLES: u32 = 3;
const PULSE_HIGH_MS: u32 = 5;
const PULSE_LOW_MS: u32 = 5;
const INTER_CYCLE_MS: u32 = 200;

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

fn init_pin_output(pin: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO via matrix)
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
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

fn pulse_n(pin: u32, n: u32) {
    for _ in 0..n {
        pin_set(pin, true);
        esp32p4_hal_testing::delay_ms(PULSE_HIGH_MS);
        pin_set(pin, false);
        esp32p4_hal_testing::delay_ms(PULSE_LOW_MS);
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_jtag_pins_as_gpio_w_logicpro");
    info!(" GPIO4 (CH6) and GPIO5 (CH7) as plain GPIO outputs");
    info!("===========================================================");
    info!("Pulse encoding: GPIO N emits N pulses per cycle.");
    info!("Cycles: {}.  GPIO4: {} pulses each.  GPIO5: {} pulses each.",
          CYCLES, PIN_A, PIN_B);
    info!("");
    info!("Logic Pro 16: digital CH1+6+7 @ 2 MS/s, threshold 1.8 V");
    info!("");

    init_pin_output(PIN_A);
    init_pin_output(PIN_B);
    pin_set(PIN_A, false);
    pin_set(PIN_B, false);

    info!("=== test_jtag_pins_as_gpio_w_logicpro: STAGE_BEGIN ===");
    for cycle in 0..CYCLES {
        info!("  cycle {}", cycle);
        pulse_n(PIN_A, PIN_A); // 4 pulses
        esp32p4_hal_testing::delay_ms(20);
        pulse_n(PIN_B, PIN_B); // 5 pulses
        esp32p4_hal_testing::delay_ms(INTER_CYCLE_MS);
    }
    pin_set(PIN_A, false);
    pin_set(PIN_B, false);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_jtag_pins_as_gpio_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_jtag_pins_as_gpio_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_jtag_pins_as_gpio_w_logicpro");
}
