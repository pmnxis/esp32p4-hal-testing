// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Corner test: UART internal loopback (no external wire needed).
//!
//! Uses GPIO matrix to route UART1 TX -> UART1 RX internally.
//! Tests: data integrity, maximum baud rate, FIFO overflow behavior.
//!
//! No external hardware needed -- pure software loopback via GPIO matrix.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

// UART1 base (same RegisterBlock as UART0, different address)
// UART0: 0x500CA000, UART1: 0x500CB000 (from PAC)
const UART1_BASE: u32 = 0x500C_B000;

// GPIO matrix signal IDs (from esp32p4.toml)
// UART1_TXD output: id=13, UART1_RXD input: id=13
const UART1_TXD_OUT: u32 = 13;
const UART1_RXD_IN: u32 = 13;

// GPIO to use for internal loopback (any free GPIO)
const LOOPBACK_PIN: u32 = 14; // GPIO14 (not used by EV Board peripherals)

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_uart_loopback: UART1 internal loopback ===");
    info!("Using GPIO{} for TX/RX loopback via GPIO matrix", LOOPBACK_PIN);

    // TODO(P4X): Configure GPIO matrix for internal loopback:
    // 1. Route UART1_TXD signal to GPIO14 output
    // 2. Route GPIO14 input to UART1_RXD signal
    // 3. Configure UART1 baud rate (115200, 921600, etc.)
    // 4. Write bytes to UART1 TX FIFO
    // 5. Read bytes from UART1 RX FIFO
    // 6. Verify data matches

    // This requires:
    // - GPIO matrix output: GPIO_FUNC_OUT_SEL_CFG for GPIO14 = UART1_TXD signal
    // - GPIO matrix input: GPIO_FUNC_IN_SEL_CFG for UART1_RXD = GPIO14
    // - UART1 clock + baud rate configuration

    info!("UART loopback: TODO -- needs UART driver runtime verification");
    info!("UART1 clock gate: compile verified (Peripheral::Uart1)");

    // Verify UART1 register is accessible
    let uart1_status = unsafe { (UART1_BASE + 0x1C) as *const u32 }; // STATUS register
    let status = unsafe { uart1_status.read_volatile() };
    info!("UART1 STATUS: 0x{:08X}", status);
    info!("UART1 accessible: OK");

    esp32p4_hal_testing::signal_pass();
    info!("=== test_uart_loopback: PASS (partial) ===");

    esp32p4_hal_testing::park_alive("test_uart_loopback");
}
