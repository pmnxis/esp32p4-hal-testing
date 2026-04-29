// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hardware-vs-Software comparison test for ESP32-P4 ROM CRC/MD5.
//!
//! Computes CRC32 / MD5 over the same inputs using both:
//!   * Hardware: silicon ROM functions via `esp_rom_sys` (linker symbols
//!     gated by `cfg(rom_crc_le)` / `cfg(rom_md5_bsd)` introduced by our
//!     esp-metadata fix for P4)
//!   * Software: pure-CPU `no_std` reference implementations
//!     (`crc-any` for CRC32, `md-5` for MD5).
//!
//! Test passes if HW result == SW reference for every input. This is the
//! end-to-end runtime proof that the ROM CRC/MD5 path on v3.2 ECO7 silicon
//! computes correct standard values, exposed correctly through our PR.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

use crc_any::CRCu32;
use md5::Digest;

/// CRC-32/ISO-HDLC (zip/gzip/PNG/Ethernet) via crc-any pre-defined.
/// Reference for esp_rom_crc32_le on ECO5+ silicon.
fn crc32_iso_hdlc_sw(data: &[u8]) -> u32 {
    let mut crc = CRCu32::crc32();
    crc.digest(data);
    crc.get_crc()
}

/// CRC-32/BZIP2 via crc-any pre-defined.
/// Reference for esp_rom_crc32_be on ECO5+ silicon.
fn crc32_bzip2_sw(data: &[u8]) -> u32 {
    let mut crc = CRCu32::crc32bzip2();
    crc.digest(data);
    crc.get_crc()
}

fn md5_sw(data: &[u8]) -> [u8; 16] {
    let mut hasher = md5::Md5::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&result);
    out
}

fn fmt_hex16(bytes: &[u8; 16]) -> [u8; 32] {
    let chars = b"0123456789abcdef";
    let mut out = [0u8; 32];
    for (i, &b) in bytes.iter().enumerate() {
        out[i * 2] = chars[(b >> 4) as usize];
        out[i * 2 + 1] = chars[(b & 0x0F) as usize];
    }
    out
}

fn check_eq_u32(label: &str, hw: u32, sw: u32) -> bool {
    let ok = hw == sw;
    if ok {
        info!("  [OK]   {}  hw=sw=0x{:08X}", label, hw);
    } else {
        info!("  [FAIL] {}  hw=0x{:08X} sw=0x{:08X}", label, hw, sw);
    }
    ok
}

fn check_eq_md5(label: &str, hw: [u8; 16], sw: [u8; 16]) -> bool {
    let ok = hw == sw;
    let hwh = fmt_hex16(&hw);
    let swh = fmt_hex16(&sw);
    let hw_str = core::str::from_utf8(&hwh).unwrap_or("?");
    let sw_str = core::str::from_utf8(&swh).unwrap_or("?");
    if ok {
        info!("  [OK]   {}  hw=sw={}", label, hw_str);
    } else {
        info!("  [FAIL] {}  hw={} sw={}", label, hw_str, sw_str);
    }
    ok
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_rom_crc_md5: HW (ROM) vs SW (CPU) comparison");
    info!(" target: ESP32-P4 v3.x silicon");
    info!("================================================");

    let mut pass = 0u32;
    let mut fail = 0u32;

    // --- CRC32 cases ---
    let cases: [(&'static str, &[u8]); 4] = [
        ("empty",      b""),
        ("\"123456789\"", b"123456789"),
        ("\"abc\"",    b"abc"),
        ("\"The quick brown fox jumps over the lazy dog\"",
                       b"The quick brown fox jumps over the lazy dog"),
    ];

    // Per IDF esp_rom_crc.h:
    //   For CRC-32/ISO-HDLC (init=~0,refin=t,refout=t,xorout=~0):
    //     standard result = crc32_le(0, buf, len)
    //   For CRC-32/BZIP2 (init=~0,refin=f,refout=f,xorout=~0):
    //     standard result = crc32_be(0, buf, len)
    info!("--- CRC32 LE (esp_rom_crc32_le == CRC-32/ISO-HDLC) ---");
    for (label, data) in cases.iter() {
        let hw = esp_rom_sys::rom::crc::crc32_le(0, data);
        let sw = crc32_iso_hdlc_sw(data);
        if check_eq_u32(label, hw, sw) { pass += 1; } else { fail += 1; }
    }

    info!("--- CRC32 BE (esp_rom_crc32_be == CRC-32/BZIP2) ---");
    for (label, data) in cases.iter() {
        let hw = esp_rom_sys::rom::crc::crc32_be(0, data);
        let sw = crc32_bzip2_sw(data);
        if check_eq_u32(label, hw, sw) { pass += 1; } else { fail += 1; }
    }

    // --- MD5 cases ---
    info!("--- MD5 (esp_rom_md5_init/update/final) ---");
    for (label, data) in cases.iter() {
        let mut ctx = esp_rom_sys::rom::md5::Context::new();
        ctx.consume(*data);
        let hw_digest = ctx.compute().0;
        let sw_digest = md5_sw(data);
        if check_eq_md5(label, hw_digest, sw_digest) { pass += 1; } else { fail += 1; }
    }

    info!("================================================");
    if fail == 0 {
        info!(" test_rom_crc_md5: PASS  ({}/{})", pass, pass + fail);
    } else {
        info!(" test_rom_crc_md5: FAIL  ({}/{} pass, {} fail)", pass, pass + fail, fail);
    }
    info!("================================================");

    esp32p4_hal_testing::park_alive("test_rom_crc_md5");
}
