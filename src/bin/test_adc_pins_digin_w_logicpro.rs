// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: which ADC1 / ADC2 pins are usable as plain *digital* inputs.
//!
//! `test_pwm_dac_loopback_w_logicpro` discovered that GPIO53 (ADC2_CH4 +
//! ANA_CMPR_CH1 reference) has its digital input buffer power-gated at
//! boot on this v3.2 silicon -- IOMUX FUN_IE = 1 + internal pull-up
//! does not bring the IN1 register bit HIGH even with the pad at 3.3 V.
//!
//! That raises the obvious follow-up: are the *other* ADC pins also
//! silenced, or is GPIO53 a one-off because it doubles as the analog
//! comparator reference? This bin sweeps every pin in:
//!
//!   ADC1_CH0..7  =  GPIO16..23
//!   ADC2_CH0..5  =  GPIO49..54
//!
//! and reports each pin's digital read-back under two configurations:
//!
//!   - **internal pull-up**  (FUN_IE=1, FUN_WPU=1)  -- expect HIGH
//!   - **internal pull-down**(FUN_IE=1, FUN_WPD=1)  -- expect LOW
//!
//! If the readback flips between the two configurations, the digital
//! input buffer is alive on that pin. If it stays stuck at the same
//! value (typically 0) for both, the input buffer is silenced.
//!
//! GPIO20 and GPIO22 are part of Group A (PWM ↔ ADC self-loop) on this
//! bench. GPIO20 is currently driven by `test_pwm_dac_loopback` if
//! flashed previously. To isolate, this bin re-initialises every
//! tested pin from scratch (function 1 = GPIO, output disabled,
//! pull-up or pull-down per phase).
//!
//! ## Logic Pro 16 setup
//!
//! Optional. Logic Pro 16 isn't strictly required for this bin -- the
//! verdict is firmware-side. But useful to confirm pad voltages line
//! up with the digital reads:
//!
//!   Digital ch 1 only @ 2 MS/s, threshold 1.8 V. (Async Serial CH1
//!   for the verdict line.)
//!
//! ## PASS criteria
//!
//! Firmware-side per pin:
//!   - phase A pull-up reads 1
//!   - phase B pull-down reads 0
//!   - difference > 0 => "digital input ALIVE"
//!
//! Bench-loaded pins (GPIO20 short to GPIO22, GPIO48 short to GPIO53)
//! may have external pull-down dominating internal pull-up; those will
//! show up as "ALIVE phase B / DEAD phase A" -- treat as inconclusive
//! and check the partner pin instead.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const ADC1_PINS: &[u32] = &[16, 17, 18, 19, 20, 21, 22, 23];
const ADC2_PINS: &[u32] = &[49, 50, 51, 52, 53, 54];

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

#[inline(always)]
fn pin_read(pin: u32) -> bool {
    unsafe {
        if pin < 32 {
            ((((GPIO_BASE + 0x3C) as *const u32).read_volatile()) >> pin) & 1 != 0
        } else {
            ((((GPIO_BASE + 0x40) as *const u32).read_volatile()) >> (pin - 32)) & 1 != 0
        }
    }
}

/// Configure pin as input only with internal pull state:
///   pull = 0 -> no pull (high-Z)
///   pull = 1 -> pull-up
///   pull = 2 -> pull-down
fn init_pin_input_with_pull(pin: u32, pull: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO)
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        let val = val | (1 << 9);                    // FUN_IE = 1
        let mut val = val & !((1 << 8) | (1 << 7));  // clear WPU/WPD first
        if pull == 1 {
            val |= 1 << 8; // FUN_WPU
        } else if pull == 2 {
            val |= 1 << 7; // FUN_WPD
        }
        r.write_volatile(val);
        // Force OEN_SEL=1 so output is truly off.
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        // Disable output enable.
        let (en_w1tc_off, bit) = if pin < 32 { (0x28u32, pin) } else { (0x34u32, pin - 32) };
        ((GPIO_BASE + en_w1tc_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

fn run_pin(pin: u32) -> (bool, bool) {
    init_pin_input_with_pull(pin, 1); // pull-up
    for _ in 0..1_000_000 { core::hint::spin_loop(); }
    let v_pu = pin_read(pin);
    init_pin_input_with_pull(pin, 2); // pull-down
    for _ in 0..1_000_000 { core::hint::spin_loop(); }
    let v_pd = pin_read(pin);
    (v_pu, v_pd)
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_adc_pins_digin_w_logicpro -- digital input survey");
    info!("===========================================================");
    info!("Per-pin: configure as input + internal pull-up, read; then");
    info!("internal pull-down, read. Pin's digital input is ALIVE iff");
    info!("the two reads differ.");
    info!("");

    let mut alive = 0u32;
    let mut dead = 0u32;
    let mut bench_loaded = 0u32;

    info!("--- ADC1 channels ---");
    for &pin in ADC1_PINS {
        let (pu, pd) = run_pin(pin);
        let tag = if pu && !pd {
            alive += 1;
            "ALIVE"
        } else if !pu && !pd {
            dead += 1;
            "DEAD (digital input silenced)"
        } else if pu && pd {
            bench_loaded += 1;
            "loaded HIGH (external pull-up?)"
        } else {
            // !pu && pd (impossible normally) or external strong pull-down
            bench_loaded += 1;
            "loaded LOW (external pull-down? bench short?)"
        };
        info!("  GPIO{:2} ADC1_CH{}  pu={} pd={}  -> {}",
              pin, pin - 16, pu as u32, pd as u32, tag);
    }

    info!("--- ADC2 channels ---");
    for &pin in ADC2_PINS {
        let (pu, pd) = run_pin(pin);
        let tag = if pu && !pd {
            alive += 1;
            "ALIVE"
        } else if !pu && !pd {
            dead += 1;
            "DEAD (digital input silenced)"
        } else if pu && pd {
            bench_loaded += 1;
            "loaded HIGH"
        } else {
            bench_loaded += 1;
            "loaded LOW"
        };
        info!("  GPIO{:2} ADC2_CH{}  pu={} pd={}  -> {}",
              pin, pin - 49, pu as u32, pd as u32, tag);
    }

    info!("");
    info!("Summary: ALIVE={} DEAD={} bench-loaded={} (total {})",
          alive, dead, bench_loaded, ADC1_PINS.len() + ADC2_PINS.len());

    esp32p4_hal_testing::signal_pass();
    info!("=== test_adc_pins_digin_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_adc_pins_digin_w_logicpro");
}
