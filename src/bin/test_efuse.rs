// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: eFuse readout -- MAC address and chip revision.
//!
//! Reads factory-programmed eFuse data.
//! No external hardware needed.
//! Pass: Valid MAC (non-zero), chip revision >= 3.0 (eco5).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_efuse: eFuse readout ===");

    // Read chip revision
    let rev = esp_hal::efuse::chip_revision();
    info!("Chip revision: {}.{}", rev.major, rev.minor);
    assert!(rev.major >= 3, "Expected eco5 (rev >= 3.0)");
    info!("Chip revision: OK (eco5+)");

    // Read MAC address via eFuse registers directly (base_mac_address needs unstable)
    // eFuse MAC0 at EFUSE base + RD_MAC_SYS_0
    let efuse_base: u32 = 0x5012_D000; // EFUSE peripheral (per esp32p4 PAC)
    let mac0 = unsafe { ((efuse_base + 0x44) as *const u32).read_volatile() }; // MAC_SYS_0
    let mac1 = unsafe { ((efuse_base + 0x48) as *const u32).read_volatile() }; // MAC_SYS_1
    info!("MAC eFuse raw: 0x{:08X}_{:08X}", mac1, mac0);
    let mac_bytes = [
        (mac1 >> 8) as u8,  // [5]
        mac1 as u8,         // [4]
        (mac0 >> 24) as u8, // [3]
        (mac0 >> 16) as u8, // [2]
        (mac0 >> 8) as u8,  // [1]
        mac0 as u8,         // [0]
    ];
    info!(
        "MAC address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac_bytes[0], mac_bytes[1], mac_bytes[2], mac_bytes[3], mac_bytes[4], mac_bytes[5]
    );
    assert!(
        mac_bytes.iter().any(|b| *b != 0),
        "MAC address should not be all zeros"
    );
    info!("MAC address: OK (non-zero)");

    esp32p4_hal_testing::signal_pass();
    info!("=== test_efuse: PASS ===");

    esp32p4_hal_testing::park_alive("test_efuse");
}
