// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: GPIO matrix output routing -- fanout UART0 TX onto an extra
//! pin, verified at byte level by Logic Pro 16.
//!
//! P4's GPIO matrix lets any peripheral output signal drive any GPIO
//! pin. esp-println configures UART0 on GPIO37 via the IO_MUX direct
//! function path (MCU_SEL = 4 selects UART0 TXD as the pad function).
//! Here we *additionally* route the same UART0_TXD signal (signal
//! index 10 in `gpio_sig_map.h`) onto GPIO27 (LA CH11) via the GPIO
//! matrix:
//!
//!   GPIO_FUNC27_OUT_SEL_CFG.OUT_SEL = 10  (UART0_TXD_PAD_OUT_IDX)
//!   GPIO_FUNC27_OUT_SEL_CFG.OEN_SEL = 0   (OE follows peripheral signal)
//!   IO_MUX_GPIO27.MCU_SEL = 1             (function 1 = GPIO via matrix)
//!   GPIO27 ENABLE = 1                     (let the matrix drive)
//!
//! After this, every byte sent to UART0 should appear *byte-identical*
//! on both GPIO37 (CH1) and GPIO27 (CH11), within ~ 1 us skew (the
//! GPIO matrix introduces one synchronizer stage on top of the IO_MUX
//! direct path).
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO37 -> LA CH1   (UART0 TX, IO_MUX direct path)
//!   GPIO27 -> LA CH11  (UART0 TX, GPIO matrix fanout)
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1, CH11
//!   Sample rate        : 5 MS/s
//!   Threshold          : 1.8 V
//!   Async Serial #1    : CH1  @ 115200 8N1
//!   Async Serial #2    : CH11 @ 115200 8N1
//!
//! ## PASS criteria
//!
//! Firmware-side: bin runs, emits payload N times, prints PASS marker.
//!
//! Host-side: both CH1 and CH11 decode identical byte streams. The
//! payload string appears at least N times on each, and the byte
//! contents match.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const FANOUT_PIN: u32 = 27;
const UART0_TXD_OUT_IDX: u32 = 10;

const ITERATIONS: u32 = 5;
const PAYLOAD: &str = "FANOUT-MARKER-XYZ";

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

/// Route `signal_idx` onto `pin` via the GPIO matrix. The output enable
/// follows the peripheral's own OE signal (OEN_SEL = 0).
fn matrix_fanout(pin: u32, signal_idx: u32) {
    unsafe {
        // IO_MUX: MCU_SEL = 1 (function 1 = GPIO via matrix), drv = 2.
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12);
        let val = (val & !(0x3 << 10)) | (2 << 10);
        r.write_volatile(val);

        // GPIO_FUNC<n>_OUT_SEL_CFG: OUT_SEL = signal_idx, OEN_SEL = 0
        // (peripheral controls OE).
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32)
            .write_volatile(signal_idx & 0x1FF);

        // ENABLE the pin so the matrix can drive it.
        let (en_w1ts_off, bit) = if pin < 32 {
            (0x24u32, pin)
        } else {
            (0x30u32, pin - 32)
        };
        ((GPIO_BASE + en_w1ts_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_gpio_matrix_uart0_fanout_w_logicpro");
    info!(" Fanout UART0 TX (sig_idx {}) onto GPIO{} (LA CH11)",
          UART0_TXD_OUT_IDX, FANOUT_PIN);
    info!("===========================================================");
    info!("Logic Pro 16: digital CH1 + CH11 @ 5 MS/s, threshold 1.8 V");
    info!("  Async Serial @ both channels @ 115200 8N1");
    info!("");

    matrix_fanout(FANOUT_PIN, UART0_TXD_OUT_IDX);

    // Brief settle so the pad takes the new role cleanly.
    esp32p4_hal_testing::delay_ms(5);

    info!("=== test_gpio_matrix_uart0_fanout_w_logicpro: STAGE_BEGIN ===");
    for i in 0..ITERATIONS {
        // The marker line travels through esp-println -> UART0 driver
        // -> UART0 TXD signal -> both IO_MUX direct (GPIO37) and GPIO
        // matrix (GPIO27) outputs simultaneously.
        info!("MARKER iter={} payload={}", i, PAYLOAD);
        esp32p4_hal_testing::delay_ms(100);
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_gpio_matrix_uart0_fanout_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_gpio_matrix_uart0_fanout_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_gpio_matrix_uart0_fanout_w_logicpro");
}
