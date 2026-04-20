// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal USB-JTAG-Serial alive test.
//!
//! Prints a single line via esp-println (jtag-serial backend) and then WFI
//! forever. Success = host sees "hello from v3.2 rust" on
//! /dev/cu.usbmodem1112101 AND the ROM USB-OTG port stays gone.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    // Boot banner -- intentionally distinctive so a cold host (that missed
    // the first message while macOS enumerated the CDC port) still sees
    // proof of life on the next heartbeat.
    println!();
    println!("==============================================");
    println!(" Boot up with rust ecosystem");
    println!(" target : ESP32-P4 v3.2 (ECO7)");
    println!(" bin    : test_usb_raw (no-HAL minimal)");
    println!("==============================================");

    let mut counter: u32 = 0;
    loop {
        println!("[heartbeat] #{counter}");
        counter = counter.wrapping_add(1);
        // crude busy-wait delay (~400 MHz, ~0.5 s)
        for _ in 0..20_000_000u32 {
            unsafe { core::arch::asm!("nop") };
        }
    }
}
