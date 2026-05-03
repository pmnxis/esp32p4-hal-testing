// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! 16 MB PRBS-31 fill + verify of PSRAM, at multiple addressing paths.
//!
//! Goal: prove the PSRAM "porting" works end-to-end by writing half of
//! the 32 MB chip with a pseudo-random pattern and reading it back.
//!
//! ESP32-P4 exposes external RAM through three address ranges (TRM Ch 7
//! "System and Memory" §7.3.3.1):
//!
//!   0x4800_0000 .. 0x4BFF_FFFF  cached + MMU mapped (the normal path)
//!   0x8800_0000 .. 0x8BFF_FFFF  uncached, accessed directly via MSPI0
//!   (0x4FF0_0000 / 0x4FFB_0000 etc are HP SRAM, NOT external RAM)
//!
//! `test_psram_shmoo` already proved the SPI-direct path (MSPI3) is
//! healthy at every (phase, delayline, drive_str, RL, LDO DREF) point
//! we can program. So a remaining fault at 0x4800_0000 must be in
//! either:
//!
//!   - the MSPI0 cache controller config (`configure_psram_mspi`), or
//!   - the cache MMU entries that translate 0x4800_0000 → physical PSRAM
//!
//! The uncached range 0x8800_0000 bypasses the MMU. If 0x88000000 works
//! but 0x48000000 doesn't → MMU bug. If neither works → MSPI0 controller
//! config bug. This test captures that distinction in one shot.
//!
//! Sequence:
//!   1. Init PSRAM via `Psram::new()`.
//!   2. Single-word probe at 0x48000000 (cache path).
//!   3. Single-word probe at 0x88000000 (bypass path).
//!   4. On whichever path works, fill 16 MB with PRBS-31 and verify.
//!
//! PRBS-31 convention: `p(x) = x^31 + x^28 + 1`, IEEE 802.3 / PCIe
//! compliance polynomial. 31-bit LFSR seeded with 0x7FFFFFFF, stepped
//! 32* per generated u32 word.
//!
//! Single-word probes use a small `try_probe` wrapper that catches the
//! exception and returns a Result rather than letting the panic handler
//! eat the program. We approximate this in user-space by writing then
//! reading a known sentinel and comparing — if the read returns a value
//! that doesn't match what we wrote, the path is broken (or the chip
//! is not actually behind that address).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::psram::{MpllFreq, Psram, PsramConfig, PsramSize, SpiRamFreq};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const PSRAM_VADDR_CACHED: u32 = 0x4800_0000;
const PSRAM_VADDR_BYPASS: u32 = 0x8800_0000;
/// 16 MB = half of the 32 MB PSRAM. Goal of the test as named.
const FILL_BYTES: usize = 16 * 1024 * 1024;
const FILL_WORDS: usize = FILL_BYTES / 4;

// ── PRBS-31 LFSR (same polynomial as test_psram_shmoo) ──
#[inline(always)]
fn prbs31_step(state: u32) -> u32 {
    let new_bit = ((state >> 30) ^ (state >> 27)) & 1;
    ((state << 1) | new_bit) & 0x7FFF_FFFF
}

#[inline(always)]
fn prbs31_word(state: &mut u32) -> u32 {
    let mut w = 0u32;
    for _ in 0..32 {
        *state = prbs31_step(*state);
        w = (w << 1) | (*state & 1);
    }
    w
}

/// Small probe: write a sentinel, read it back. Used to detect whether
/// a given virtual address actually returns to the configured PSRAM
/// without raising a bus fault. Note: a *bus error* on access cannot be
/// caught here — it pulls the CPU into the exception handler. We rely
/// on observing whether this function returns at all (vs a panic from
/// the exception handler) plus whether the readback matches.
fn try_probe(vaddr: u32, sentinel: u32) -> Result<u32, ()> {
    unsafe {
        (vaddr as *mut u32).write_volatile(sentinel);
        let v = (vaddr as *const u32).read_volatile();
        if v == sentinel { Ok(v) } else { Err(()) }
    }
}

/// Words per progress message. 1 MB worth of words.
const PROGRESS_WORDS: usize = 1024 * 1024 / 4;

/// "Pet the watchdog" stub. We don't have direct WDT control here, but
/// we keep a small `nop` loop between batches to reduce the chance of
/// a hardware WDT firing during long fills.
#[inline(never)]
fn nop_breath() {
    for _ in 0..32 {
        core::hint::spin_loop();
    }
}

fn write_fill(start: u32, words: usize, seed: u32) {
    let mut s = seed;
    let p = start as *mut u32;
    let mut written = 0usize;
    while written < words {
        let next = core::cmp::min(written + PROGRESS_WORDS, words);
        for i in written..next {
            let w = prbs31_word(&mut s);
            unsafe { p.add(i).write_volatile(w) };
        }
        written = next;
        info!(
            "    wrote {:5} / {:5} KB",
            written * 4 / 1024,
            words * 4 / 1024
        );
        nop_breath();
    }
}

fn verify_fill(start: u32, words: usize, seed: u32, max_report: u32) -> u32 {
    let mut s = seed;
    let p = start as *const u32;
    let mut mismatches = 0u32;
    let mut reported = 0u32;
    let mut read = 0usize;
    while read < words {
        let next = core::cmp::min(read + PROGRESS_WORDS, words);
        for i in read..next {
            let want = prbs31_word(&mut s);
            let got = unsafe { p.add(i).read_volatile() };
            if got != want {
                mismatches += 1;
                if reported < max_report {
                    info!(
                        "    mismatch @ 0x{:08X}: got=0x{:08X} want=0x{:08X}",
                        start + (i * 4) as u32,
                        got,
                        want,
                    );
                    reported += 1;
                }
            }
        }
        read = next;
        info!(
            "    read   {:5} / {:5} KB ({} mismatches so far)",
            read * 4 / 1024,
            words * 4 / 1024,
            mismatches
        );
        nop_breath();
    }
    mismatches
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_psram_prbs_fill — 16 MB PRBS-31 fill + verify");
    info!("================================================");

    info!("[1/5] Init PSRAM at 250 MHz (MPLL=500, div=2) — non-default top speed");
    // ram_frequency=Mhz250 selects the 250-MHz timing row (MR0.RL=6,
    // MR4.WL=3, RD dummy=34). core_clock=Mhz500 is the matching MPLL
    // override (the table default for Mhz250 is also 500 MHz; this
    // exercises the explicit override path).
    let cfg = PsramConfig {
        size: PsramSize::AutoDetect,
        ram_frequency: SpiRamFreq::Mhz250,
        core_clock: Some(MpllFreq::Mhz500),
        ..PsramConfig::default()
    };
    let psram = Psram::new(peripherals.PSRAM, cfg);
    let (ptr, size) = psram.raw_parts();
    info!(
        "  PSRAM range: {:p} .. {:p}  ({} MB)",
        ptr,
        unsafe { ptr.add(size) },
        size / 1024 / 1024
    );

    // ── Diagnostic: dump MSPI0 cache controller registers BEFORE probing ──
    //
    // If something is off in `configure_psram_mspi`, this is where we
    // see it. Compare to IDF v6.1 baseline: after `s_config_mspi_for_psram`
    // we'd expect:
    //   CACHE_SCTRL  bits set for cache_usr_saddr_4byte, usr_*_sram_dummy,
    //                cache_sram_usr_rcmd/wcmd, sram_oct, sram_addr_bitlen=31,
    //                rdummy=25, wdummy=11
    //   SRAM_CMD     scmd_oct, saddr_oct, sdin_oct, sdout_oct, sdin_hex,
    //                sdout_hex, sdummy_wout
    //   CACHE_FCTRL  bit 0  AXI_REQ_EN          = 1
    //                bit 31 CLOSE_AXI_INF_EN   = 0  (POR default is 1!)
    //   SMEM_DDR     bit 0  smem_ddr_en        = 1
    //                bit 1  smem_var_dummy     = 1
    info!("[2/4] MSPI0 cache controller registers after Psram::new():");
    let mspi0 = 0x5008_E000_u32;
    let dump = |name: &str, off: u32| {
        let v = unsafe { ((mspi0 + off) as *const u32).read_volatile() };
        info!("  {:14} (0x{:03X}) = 0x{:08X}", name, off, v);
    };
    dump("CACHE_FCTRL", 0x3C);
    dump("CACHE_SCTRL", 0x40);
    dump("SRAM_CMD", 0x44);
    dump("SRAM_DRD_CMD", 0x48);
    dump("SRAM_DWR_CMD", 0x4C);
    dump("SRAM_CLK", 0x50);
    dump("MEM_CTRL1", 0x70);
    dump("SMEM_DDR", 0xD8);
    dump("SMEM_AC", 0x1A0);

    // ── Tiny incremental probes (BISECTING WRITE HANG) ──
    info!("[3/5] Tiny incremental cached writes/reads — bisect hang location");
    let p = PSRAM_VADDR_CACHED as *mut u32;
    info!("  step 1: write 1 word at 0x48000000");
    unsafe { p.write_volatile(0x1111_1111) };
    info!("  step 2: read back");
    let v = unsafe { p.read_volatile() };
    info!("    read=0x{:08X}", v);
    info!("  step 3: write 16 sequential words (1 cache line)");
    for i in 0..16 {
        unsafe { p.add(i).write_volatile(0x2000_0000 | i as u32) };
    }
    info!("    16-word write OK");
    info!("  step 4: read 16 sequential words");
    for i in 0..16 {
        let _ = unsafe { p.add(i).read_volatile() };
    }
    info!("    16-word read OK");
    info!("  step 5: write 256 sequential words (16 cache lines)");
    for i in 0..256 {
        unsafe { p.add(i).write_volatile(0x3000_0000 | i as u32) };
    }
    info!("    256-word write OK");
    info!("  step 6: read 256 sequential words");
    for i in 0..256 {
        let _ = unsafe { p.add(i).read_volatile() };
    }
    info!("    256-word read OK");

    info!("[4/5] Probe addressing paths");
    info!(
        "  Probing BYPASS path 0x{:08X} first (no MMU, no cache)...",
        PSRAM_VADDR_BYPASS
    );
    info!("    if this faults -> MSPI0 cache-controller config is wrong");
    info!("    if this passes -> chip + MSPI0 paths fine");
    let bypass_ok = try_probe(PSRAM_VADDR_BYPASS, 0xCAFE_BABE).is_ok();
    info!("  bypass path: {}", if bypass_ok { "OK" } else { "MISMATCH" });

    info!(
        "  Probing CACHED path 0x{:08X} (MMU + L1/L2)...",
        PSRAM_VADDR_CACHED
    );
    info!("    NOTE: if MMU is broken, this faults and the program panics.");
    info!("    Reaching the next line means cached path works too.");
    let cached_ok = try_probe(PSRAM_VADDR_CACHED, 0xDEAD_BEEF).is_ok();
    info!("  cached path: {}", if cached_ok { "OK" } else { "MISMATCH" });

    info!("[4/4] Decision matrix");
    let fill_addr = match (cached_ok, bypass_ok) {
        (true, _) => {
            info!("  cache + MMU + MSPI0 controller all WORKING -- using cached path");
            Some(PSRAM_VADDR_CACHED)
        }
        (false, true) => {
            info!("  bypass works but cached mismatched -- MMU or cache controller bug");
            info!("  using bypass path for the fill (validates chip + MSPI0)");
            Some(PSRAM_VADDR_BYPASS)
        }
        (false, false) => {
            info!("  BOTH paths fail -- MSPI0 cache-controller config is wrong");
            info!("  (SPI-direct path on MSPI3 still works, see test_psram_shmoo)");
            None
        }
    };

    info!("[5/5] 16 MB PRBS-31 fill + verify");
    if let Some(addr) = fill_addr {
        info!("  writing  16 MB at 0x{:08X} with PRBS-31 seed 0x7FFFFFFF...", addr);
        write_fill(addr, FILL_WORDS, 0x7FFF_FFFF);
        info!("  verifying...");
        let mismatches = verify_fill(addr, FILL_WORDS, 0x7FFF_FFFF, 8);
        info!(
            "  result: {} / {} words mismatched",
            mismatches, FILL_WORDS
        );
        if mismatches == 0 {
            info!(" test_psram_prbs_fill: PASS");
            esp32p4_hal_testing::signal_pass();
        } else {
            info!(" test_psram_prbs_fill: FAIL ({} mismatches)", mismatches);
        }
    } else {
        info!(" test_psram_prbs_fill: FAIL (no working path to PSRAM)");
    }

    info!("================================================");
    esp32p4_hal_testing::park_alive("test_psram_prbs_fill");
}
