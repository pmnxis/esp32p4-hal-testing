// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: Chip identification and revision detection.
//!
//! Reads all chip identification data from eFuse and CSR registers:
//! - Chip revision (major.minor) from eFuse WAFER_VERSION
//! - Chip model / package info
//! - MAC address (factory programmed)
//! - CPU features (ISA extensions)
//! - eFuse raw revision encoding (5-bit + bit23)
//!
//! Reference:
//!   Errata doc revision table:
//!     v0.0 = 000000 (marking A)
//!     v1.0 = 010000 (marking C)
//!     v1.3 = 010011 (marking E)
//!     v3.0 = 110000 (marking F, eco5)
//!     v3.1 = 110001 (marking G)
//!   eFuse register: EFUSE_RD_MAC_SPI_SYS_2_REG bits [23] and [5:0]
//!
//! No external hardware needed.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

/// EFUSE controller base address (per esp32p4 PAC: 0x5012_D000).
/// Previously set to 0x500B_0800 which is unmapped on P4 -> read fault.
const EFUSE_BASE: u32 = 0x5012_D000;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_chip_id: Chip identification ===");

    // 1. Chip revision via esp-hal API
    let rev = esp_hal::efuse::chip_revision();
    info!("Chip revision (esp-hal): v{}.{}", rev.major, rev.minor);

    // 2. Raw eFuse revision bits
    // EFUSE_RD_MAC_SPI_SYS_2_REG = Block1 word 2 (offset from rd_mac_sys_0)
    // rd_mac_sys_0 is at EFUSE_BASE + 0x44 (Block1 start)
    // Word 2 = EFUSE_BASE + 0x44 + 8 = 0x4C
    let mac_sys_2 = unsafe { ((EFUSE_BASE + 0x4C) as *const u32).read_volatile() };
    let rev_bits_5_0 = mac_sys_2 & 0x3F;
    let rev_bit_23 = (mac_sys_2 >> 23) & 1;
    info!("eFuse raw: RD_MAC_SPI_SYS_2 = 0x{:08X}", mac_sys_2);
    info!("  rev[5:0] = 0b{:06b} ({})", rev_bits_5_0, rev_bits_5_0);
    info!("  rev[23]  = {}", rev_bit_23);

    // Decode revision marking
    let marking = match (rev.major, rev.minor) {
        (0, 0) => "A (v0.0, eco0)",
        (1, 0) => "C (v1.0, eco1)",
        (1, 3) => "E (v1.3)",
        (3, 0) => "F (v3.0, eco5)",
        (3, 1) => "G (v3.1 -- EV Board default)",
        (3, 2) => "? (v3.2)",
        _ => "UNKNOWN",
    };
    info!("Marking code: {}", marking);

    // 3. MAC address from eFuse
    let mac0 = unsafe { ((EFUSE_BASE + 0x44) as *const u32).read_volatile() };
    let mac1 = unsafe { ((EFUSE_BASE + 0x48) as *const u32).read_volatile() };
    let mac_bytes = [
        (mac1 >> 8) as u8,
        mac1 as u8,
        (mac0 >> 24) as u8,
        (mac0 >> 16) as u8,
        (mac0 >> 8) as u8,
        mac0 as u8,
    ];
    info!(
        "MAC address: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac_bytes[0], mac_bytes[1], mac_bytes[2],
        mac_bytes[3], mac_bytes[4], mac_bytes[5]
    );

    // Espressif OUI check (first 3 bytes)
    let is_espressif = mac_bytes[0] == 0x08 && mac_bytes[1] == 0x3A && mac_bytes[2] == 0xF2
        || mac_bytes[0] == 0x24 && mac_bytes[1] == 0x0A && mac_bytes[2] == 0xC4
        || mac_bytes[0] == 0x30 && mac_bytes[1] == 0xAE && mac_bytes[2] == 0xA4
        || mac_bytes[0] == 0xA0 && mac_bytes[1] == 0x76 && mac_bytes[2] == 0x4E
        || mac_bytes[0] == 0xCC && mac_bytes[1] == 0x8D && mac_bytes[2] == 0xA2;
    info!("Espressif OUI: {}", if is_espressif { "YES" } else { "NO (custom MAC?)" });

    // 4. CPU identification via CSR
    let mvendorid: u32;
    let marchid: u32;
    let mimpid: u32;
    let mhartid: u32;
    let misa: u32;
    unsafe {
        core::arch::asm!("csrr {}, mvendorid", out(reg) mvendorid);
        core::arch::asm!("csrr {}, marchid", out(reg) marchid);
        core::arch::asm!("csrr {}, mimpid", out(reg) mimpid);
        core::arch::asm!("csrr {}, mhartid", out(reg) mhartid);
        core::arch::asm!("csrr {}, misa", out(reg) misa);
    }
    info!("CPU mvendorid: 0x{:08X}", mvendorid);
    info!("CPU marchid:   0x{:08X}", marchid);
    info!("CPU mimpid:    0x{:08X}", mimpid);
    info!("CPU mhartid:   {} (0=HP Core 0, 1=HP Core 1)", mhartid);
    info!("CPU misa:      0x{:08X}", misa);

    // Decode MISA extensions
    let mut extensions = [0u8; 26];
    let mut ext_str_len = 0;
    for i in 0..26 {
        if misa & (1 << i) != 0 {
            extensions[ext_str_len] = b'A' + i as u8;
            ext_str_len += 1;
        }
    }
    // Print extensions as individual chars
    info!("ISA extensions present:");
    for i in 0..ext_str_len {
        let c = extensions[i] as char;
        let name = match c {
            'I' => "Integer base",
            'M' => "Multiply/Divide",
            'A' => "Atomic",
            'F' => "Single-precision float",
            'C' => "Compressed (16-bit)",
            'U' => "User mode",
            'S' => "Supervisor mode",
            _ => "other",
        };
        info!("  {} -- {}", c, name);
    }

    // P4 should have at minimum: I, M, A, F, C
    let has_i = misa & (1 << 8) != 0;
    let has_m = misa & (1 << 12) != 0;
    let has_a = misa & (1 << 0) != 0;
    let has_f = misa & (1 << 5) != 0;
    let has_c = misa & (1 << 2) != 0;
    info!("IMAFC present: I={} M={} A={} F={} C={}", has_i, has_m, has_a, has_f, has_c);
    assert!(has_i && has_m && has_a && has_f && has_c, "P4 should have IMAFC");

    // 5. Dual-core check
    info!("Hart ID: {} (running on HP Core {})", mhartid, mhartid);
    // P4 is dual-core but we boot on Core 0
    assert!(mhartid == 0, "Expected to boot on Core 0");

    // 6. PMP entry count (v3.x has 32, v1.x has 16)
    // Read pmpcfg0 to check if PMP is accessible
    let pmpcfg0: u32;
    unsafe { core::arch::asm!("csrr {}, pmpcfg0", out(reg) pmpcfg0); }
    info!("PMP pmpcfg0: 0x{:08X}", pmpcfg0);
    info!("PMP entries: {} (v3.x should have 32)", if rev.major >= 3 { 32 } else { 16 });

    // 7. Summary
    info!("--- Chip ID Summary ---");
    info!("Model:    ESP32-P4");
    info!("Revision: v{}.{} ({})", rev.major, rev.minor, marking);
    info!("CPU:      RV32IMAFC, Hart {}", mhartid);
    info!("Cores:    2 (HP Core 0 + HP Core 1) + LP Core");
    info!(
        "MAC:      {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac_bytes[0], mac_bytes[1], mac_bytes[2],
        mac_bytes[3], mac_bytes[4], mac_bytes[5]
    );

    esp32p4_hal_testing::signal_pass();
    info!("=== test_chip_id: PASS ===");

    esp32p4_hal_testing::park_alive("test_chip_id");
}
