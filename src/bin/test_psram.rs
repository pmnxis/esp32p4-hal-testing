// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PSRAM smoke test (register-level).
//!
//! On ESP32-P4X (v3.x ECO5+), PSRAM is connected via the dedicated AP HEX
//! PSRAM controller (`PSRAM_MSPI0` at 0x5008_E000) and mapped to virtual
//! address 0x4800_0000+ through the cache MMU.
//!
//! What this test does:
//!
//! 1. Probe PSRAM mode register MR2 via direct SPI command on
//!    PSRAM_MSPI0. The MR2 density bits decode to a known PSRAM size
//!    (per `esp-idf esp_psram_impl_ap_hex.c`).
//! 2. If MR2 looks valid, attempt a write+read at PSRAM_VADDR_START
//!    (0x4800_0000) to confirm the cache MMU is mapping the region.
//! 3. Pattern test 4 KB to catch addressing/alias bugs.
//!
//! Expected outcomes:
//!
//! - If the IDF v6.0.1 bootloader handed off with PSRAM already
//!   initialized: density read should succeed, vaddr probe should pass.
//! - If the bootloader did NOT enable PSRAM: density read returns 0xFF
//!   (MSPI idle), vaddr probe likely faults. That confirms
//!   `esp-hal::psram::implem::init_psram` (the WIP P4 stub) is required
//!   to actually bring PSRAM up, which it does not yet do.
//!
//! NOTE: This is a stand-alone register-level test. We deliberately do
//! NOT depend on `esp-hal::psram::Psram` because the P4 driver requires
//! the `unstable` feature, which currently exposes 46 unrelated
//! P4-driver build errors (separate maintenance work).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

// === Register addresses (P4 v3.x / ECO5+) ===
// IDF v6.0.1 hw_ver3/soc/reg_base.h: DR_REG_PSRAM_MSPI0_BASE
const PSRAM_MSPI0_BASE: u32 = 0x5008_E000;

// SPIMEM-S register offsets (subset, from `spi_mem_s_reg.h`)
const SPI_CMD: u32 = 0x00;
const SPI_ADDR: u32 = 0x04;
const SPI_USER: u32 = 0x18;
const SPI_USER1: u32 = 0x1C;
const SPI_USER2: u32 = 0x20;
const SPI_MISO_DLEN: u32 = 0x28;
const SPI_W0: u32 = 0x58;

const USR_COMMAND: u32 = 1 << 31;
const USR_ADDR: u32 = 1 << 30;
const USR_DUMMY: u32 = 1 << 29;
const USR_MISO: u32 = 1 << 28;
const CMD_USR: u32 = 1 << 18;

// AP HEX PSRAM transaction parameters (esp-idf
// `esp_psram_impl_ap_hex.c::AP_HEX_PSRAM_*`).
const AP_HEX_REG_READ_CMD: u32 = 0x4040;
const CMD_BITLEN: u32 = 16;
const ADDR_BITLEN: u32 = 32;
const RD_REG_DUMMY_BITLEN: u32 = 16; // 2 * (9 - 1)
const DATA_BITLEN: u32 = 16; // MR2 + MR3 packed

// PSRAM virtual mapping
const PSRAM_VADDR_START: u32 = 0x4800_0000;

// === HP_SYS_CLKRST registers (clock gates for PSRAM core) ===
// Per IDF master commit 8a536eeda0 ("fix(mspi): enable PSRAM core clock"),
// the v6.0.1 bootloader path can leave reg_psram_core_clk_en cleared,
// which makes SPIMEM2 fault and MR reads return 0x00. We inspect and fix
// all three PSRAM clock-gate bits before attempting MR2 read.
const HP_SYS_CLKRST_BASE: u32 = 0x500E_6000;
const SOC_CLK_CTRL0: u32 = 0x14; // bit 31 = reg_psram_sys_clk_en
const PERI_CLK_CTRL00: u32 = 0x30; // bit 14 = pll_clk_en, bit 15 = core_clk_en
const PSRAM_SYS_CLK_EN_BIT: u32 = 1 << 31;
const PSRAM_PLL_CLK_EN_BIT: u32 = 1 << 14;
const PSRAM_CORE_CLK_EN_BIT: u32 = 1 << 15;

fn mmio_r(addr: u32) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}
fn mmio_w(addr: u32, val: u32) {
    unsafe { (addr as *mut u32).write_volatile(val) }
}

/// Issue PSRAM_MSPI0 register read at addr=0x2 (returns MR2+MR3 packed).
fn read_mr2_mr3() -> Option<u16> {
    mmio_w(
        PSRAM_MSPI0_BASE + SPI_USER2,
        ((CMD_BITLEN - 1) << 28) | AP_HEX_REG_READ_CMD,
    );
    mmio_w(
        PSRAM_MSPI0_BASE + SPI_USER1,
        ((ADDR_BITLEN - 1) << 26) | (RD_REG_DUMMY_BITLEN - 1),
    );
    mmio_w(PSRAM_MSPI0_BASE + SPI_ADDR, 0x2);
    mmio_w(PSRAM_MSPI0_BASE + SPI_MISO_DLEN, DATA_BITLEN - 1);
    mmio_w(
        PSRAM_MSPI0_BASE + SPI_USER,
        USR_COMMAND | USR_ADDR | USR_DUMMY | USR_MISO,
    );
    mmio_w(PSRAM_MSPI0_BASE + SPI_CMD, CMD_USR);

    let mut timeout = 100_000u32;
    while mmio_r(PSRAM_MSPI0_BASE + SPI_CMD) & CMD_USR != 0 {
        timeout -= 1;
        if timeout == 0 {
            return None;
        }
    }
    Some((mmio_r(PSRAM_MSPI0_BASE + SPI_W0) & 0xFFFF) as u16)
}

fn density_to_size(density: u8) -> Option<usize> {
    match density {
        0x1 => Some(4 * 1024 * 1024),  // 32 Mbit
        0x3 => Some(8 * 1024 * 1024),  // 64 Mbit
        0x5 => Some(16 * 1024 * 1024), // 128 Mbit
        0x6 => Some(64 * 1024 * 1024), // 512 Mbit (per IDF density chain)
        0x7 => Some(32 * 1024 * 1024), // 256 Mbit
        _ => None,
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_psram (register-level smoke test)");
    info!(" target: ESP32-P4X v3.x silicon, AP HEX PSRAM");
    info!(" controller: PSRAM_MSPI0 @ 0x{:08X}", PSRAM_MSPI0_BASE);
    info!(" virtual map: 0x{:08X}+", PSRAM_VADDR_START);
    info!("================================================");

    info!("[0/3] Inspect + force HP_SYS_CLKRST PSRAM clock gates");
    let soc0_before = mmio_r(HP_SYS_CLKRST_BASE + SOC_CLK_CTRL0);
    let peri0_before = mmio_r(HP_SYS_CLKRST_BASE + PERI_CLK_CTRL00);
    let sys_en = (soc0_before & PSRAM_SYS_CLK_EN_BIT) != 0;
    let pll_en = (peri0_before & PSRAM_PLL_CLK_EN_BIT) != 0;
    let core_en = (peri0_before & PSRAM_CORE_CLK_EN_BIT) != 0;
    info!(
        "  before: sys_clk_en={}, pll_clk_en={}, core_clk_en={}",
        sys_en as u8, pll_en as u8, core_en as u8
    );
    if !sys_en || !pll_en || !core_en {
        info!("  forcing all three PSRAM clock gates ON (per IDF 8a536eeda0)");
        mmio_w(
            HP_SYS_CLKRST_BASE + SOC_CLK_CTRL0,
            soc0_before | PSRAM_SYS_CLK_EN_BIT,
        );
        mmio_w(
            HP_SYS_CLKRST_BASE + PERI_CLK_CTRL00,
            peri0_before | PSRAM_PLL_CLK_EN_BIT | PSRAM_CORE_CLK_EN_BIT,
        );
    }

    info!("[1/3] Read MR2/MR3 via AP_HEX_PSRAM_REG_READ (0x4040)");
    let mr_pair = match read_mr2_mr3() {
        Some(v) => v,
        None => {
            info!("  TIMEOUT - PSRAM controller not responding");
            info!("=== test_psram: FAIL (MSPI hung) ===");
            esp32p4_hal_testing::park_alive("test_psram");
        }
    };
    let mr2 = (mr_pair & 0xFF) as u8;
    let mr3 = (mr_pair >> 8) as u8;
    info!("  MR2 = 0x{:02X}, MR3 = 0x{:02X}", mr2, mr3);

    let density = mr2 & 0x7;
    let dev_id = (mr2 >> 3) & 0x3;
    let kgd = (mr2 >> 5) & 0x7;
    info!("  density bits[2:0] = 0x{:X}", density);
    info!(
        "  dev_id  bits[4:3] = 0x{:X} (generation {})",
        dev_id,
        dev_id + 1
    );
    info!(
        "  kgd     bits[7:5] = 0x{:X} ({})",
        kgd,
        if kgd == 6 { "Pass" } else { "Fail/unknown" }
    );

    let size_bytes = match density_to_size(density) {
        Some(s) => {
            info!("  decoded PSRAM size: {} MB", s / 1024 / 1024);
            s
        }
        None => {
            info!("  unknown density value -- PSRAM likely not initialized");
            info!("=== test_psram: FAIL (density unknown) ===");
            esp32p4_hal_testing::park_alive("test_psram");
        }
    };

    info!(
        "[2/3] Probe PSRAM vaddr (write + read at 0x{:08X})",
        PSRAM_VADDR_START
    );
    let probe_word: u32 = 0xCAFE_BABE;
    let vaddr = PSRAM_VADDR_START as *mut u32;
    let read_back: u32;
    unsafe {
        vaddr.write_volatile(probe_word);
        read_back = vaddr.read_volatile();
    }
    info!("  wrote 0x{:08X}, read 0x{:08X}", probe_word, read_back);
    let probe_ok = read_back == probe_word;

    info!("[3/3] Pattern test over first 4 KB of PSRAM");
    let mut mismatches = 0u32;
    if probe_ok {
        unsafe {
            let p = PSRAM_VADDR_START as *mut u32;
            for i in 0..1024u32 {
                p.add(i as usize)
                    .write_volatile(i.wrapping_mul(0x9E37_79B9));
            }
            for i in 0..1024u32 {
                let want = i.wrapping_mul(0x9E37_79B9);
                let got = p.add(i as usize).read_volatile();
                if got != want {
                    mismatches += 1;
                }
            }
        }
        info!("  {} / 1024 words mismatched", mismatches);
    } else {
        info!("  skipped (vaddr probe failed)");
    }

    info!("================================================");
    let detected_mb = size_bytes / 1024 / 1024;
    if probe_ok && mismatches == 0 {
        info!(
            " test_psram: PASS  ({} MB AP HEX PSRAM is read/writeable at 0x{:08X})",
            detected_mb, PSRAM_VADDR_START
        );
        esp32p4_hal_testing::signal_pass();
    } else if !probe_ok {
        info!(" test_psram: FAIL  (vaddr probe mismatch -- PSRAM likely not mapped)");
        info!("              MR2 read worked, but virtual address access does not.");
        info!("              Bootloader probably did not bring PSRAM up.");
    } else {
        info!(" test_psram: FAIL  ({} mismatches over 4KB)", mismatches);
    }
    info!("================================================");

    esp32p4_hal_testing::park_alive("test_psram");
}
