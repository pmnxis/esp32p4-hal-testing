// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! AP HEX PSRAM DQS / delayline shmoo for ESP32-P4.
//!
//! Sweeps the two-dimensional training space exposed by the SoC's MSPI
//! IOMUX (see `docs/PSRAM_AP_HEX_REFERENCE.md` for the chip-side MR
//! map and how this differs from JEDEC DDR training):
//!
//!   Y-axis (4 points):   DQS phase  =  67.5° / 78.75° / 90° / 101.25°
//!   X-axis (16 points):  DQS+data delayline  =  0..15 (~30 ps each)
//!
//! For each (phase, delayline) point we:
//!   1. Set the IOMUX phase + delayline registers.
//!   2. Write a 64-byte reference pattern into PSRAM at 0x80 via direct
//!      MSPI3 SYNC_WRITE (does NOT depend on the cache-side AXI path
//!      working).
//!   3. Read the same range back via MSPI3 SYNC_READ.
//!   4. Compare. Mark the cell `P` (pass) or `.` (fail).
//!
//! Then we print an ASCII shmoo grid, pick the longest passing
//! consecutive run on each axis (matching IDF
//! `mspi_timing_psram_select_best_tuning_*`), and apply the best
//! (phase, delayline) to the live IOMUX registers. Finally we attempt
//! a cache-side AXI read at 0x4800_0000 to see if the calibrated
//! timing makes the L1+L2 → MSPI0 cache fill path work.
//!
//! Why not use the IDF `mspi_timing_psram_tuning` API: this binary is
//! a from-scratch reimplementation in `no_std` Rust that relies only
//! on (a) the same ROM helpers IDF uses for SPI command setup
//! (`esp_rom_spi_set_op_mode`/`cmd_config`), and (b) raw IOMUX writes.
//! It exists primarily to *visualize* the chip's timing margin on a
//! given board, which the IDF API does not expose -- IDF returns only
//! the chosen best point, not the full grid.

#![no_std]
#![no_main]

use core::fmt::Write;

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::psram::{Psram, PsramConfig, PsramSize};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

// ── PSRAM_MSPI1 register constants (identical to esp-hal psram driver) ──
const MSPI1_BASE: u32 = 0x5008_F000;
const MSPI1_CMD: u32 = MSPI1_BASE + 0x00;
const MSPI1_W0: u32 = MSPI1_BASE + 0x58;
const SPI_USR_TRIGGER: u32 = 1 << 18;

// ── IOMUX_MSPI_PIN base + per-pin register offsets ──
const IOMUX_MSPI_BASE: u32 = 0x500E_1200;
const PSRAM_DQS_0_REG: u32 = IOMUX_MSPI_BASE + 0x3C;
const PSRAM_DQS_1_REG: u32 = IOMUX_MSPI_BASE + 0x68;

/// PSRAM data/clk/cs pin register offsets (relative to IOMUX_MSPI_BASE).
/// Same set as `psram_pad_init` in the driver, minus the DQS regs which
/// have their own delayline encoding.
const DATA_PIN_OFFSETS: &[u32] = &[
    0x1C, // PSRAM_D
    0x20, // PSRAM_Q
    0x24, // PSRAM_WP
    0x28, // PSRAM_HOLD
    0x2C, // PSRAM_DQ4
    0x30, // PSRAM_DQ5
    0x34, // PSRAM_DQ6
    0x38, // PSRAM_DQ7
    0x40, // PSRAM_DQ8
    0x44, // PSRAM_DQ9
    0x48, // PSRAM_DQ10
    0x4C, // PSRAM_DQ11
    0x50, // PSRAM_DQ12
    0x54, // PSRAM_DQ13
    0x58, // PSRAM_DQ14
    0x5C, // PSRAM_DQ15
    0x60, // PSRAM_CK
    0x64, // PSRAM_CS
];

// ── Stress test patterns ──
//
// Each grid cell is verified with multiple patterns; cell passes only if
// all patterns match. The patterns are chosen to expose different
// failure classes:
//
//   ALL_0      single-bit-stuck-high anywhere on the bus
//   ALL_1      single-bit-stuck-low anywhere on the bus
//   ALT_55_AA  toggling every bit each cycle; max bit-flip rate, exposes
//              crosstalk + DQS/data deskew
//   ALT_16     toggling every 16-bit half independently — directly
//              stresses Hex (x16) mode line splitting
//   WALK_1S    only one bit set per word, scanning bit 0 .. 31; the
//              strictest check for per-line addressing (any swapped pair
//              shows up as the "wrong bit" in readback)
//   IDF_REF    Espressif's `mspi_timing_by_dqs.c::s_test_data` pattern —
//              kept for compatibility with IDF's own training procedure
//
// Burst-write granularity is `BURST_LEN` bytes per cell: we issue
// `BURST_LEN / FIFO_MAX` SPI writes to consecutive addresses, then a
// matching number of reads, and compare. With BURST_LEN = 256 the test
// crosses both AP HEX PSRAM 32-byte and 64-byte burst boundaries to
// detect any address-LSB or page-rollover bug.
const PSRAM_TEST_ADDR: u32 = 0x80;
const FIFO_MAX: usize = 64; // PSRAM_CTRLR_LL_FIFO_MAX_BYTES
const BURST_LEN: usize = 256; // = 4 * FIFO_MAX, crosses 64-B / 128-B boundaries
const BURST_CHUNKS: usize = BURST_LEN / FIFO_MAX; // = 4

#[derive(Copy, Clone)]
enum Pattern {
    All0,
    All1,
    Alt5555Aaaa,
    Alt16,
    Walk1s,
    IdfRef,
    /// PRBS-31: pseudo-random bit sequence per IEEE 802.3 / PCIe / USB
    /// compliance, polynomial `p(x) = x^31 + x^28 + 1`. Maximal-length
    /// LFSR (period = 2^31 − 1 ≈ 2.1 G bits) with near-uniform 1/0
    /// density and flat power spectrum — the gold-standard "torture"
    /// pattern for detecting any bit-position bias, swapped DQ pairs,
    /// or DQS gating misalignment that lower-entropy patterns might
    /// miss. Initialized from a fixed seed (0x7FFFFFFF) so reads and
    /// writes generate the same sequence; the LFSR is stepped 32*
    /// per generated u32 word.
    Prbs31,
}

const PATTERNS: &[Pattern] = &[
    Pattern::All0,
    Pattern::All1,
    Pattern::Alt5555Aaaa,
    Pattern::Alt16,
    Pattern::Walk1s,
    Pattern::IdfRef,
    Pattern::Prbs31,
];

/// Single PRBS-31 LFSR step. Returns the new state. Polynomial
/// `x^31 + x^28 + 1` matches IEEE 802.3 / PCIe convention; taps are
/// bits 30 and 27 of the 31-bit shift register (0-indexed).
#[inline(always)]
fn prbs31_step(state: u32) -> u32 {
    let new_bit = ((state >> 30) ^ (state >> 27)) & 1;
    ((state << 1) | new_bit) & 0x7FFF_FFFF
}

/// Generate one 32-bit PRBS-31 word by stepping the LFSR 32 times,
/// MSB-first (so bit i of the word = LFSR output at step i).
#[inline(always)]
fn prbs31_word(state: &mut u32) -> u32 {
    let mut w = 0u32;
    for _ in 0..32 {
        *state = prbs31_step(*state);
        w = (w << 1) | (*state & 1);
    }
    w
}

#[rustfmt::skip]
static IDF_REF_DATA: [u32; FIFO_MAX / 4] = [
    0x7f78_6655, 0xa5ff_005a, 0x3f3c_33aa, 0xa5ff_5a00,
    0x1f1e_9955, 0xa500_5aff, 0x0f0f_ccaa, 0xa55a_00ff,
    0x0787_6655, 0xffa5_5a00, 0x03c3_33aa, 0xff00_a55a,
    0x01e1_9955, 0xff00_5aa5, 0x00f0_ccaa, 0xff5a_00a5,
];

/// Fill `buf` (BURST_LEN bytes, u32-aligned) with the given pattern.
/// `start_word` = the index of the first u32 in the larger logical
/// stream (used by walking-1s and address-as-data style patterns to
/// produce continuous content across burst chunks).
fn fill_pattern(buf: &mut [u8; BURST_LEN], pat: Pattern, start_word: u32) {
    let words = BURST_LEN / 4;

    // PRBS-31 needs a stateful LFSR; advance the seed past `start_word`
    // generated words so writes and reads of the same logical offset
    // produce the same byte sequence even across multiple chunks.
    let mut prbs_state: u32 = 0x7FFF_FFFF;
    if matches!(pat, Pattern::Prbs31) {
        for _ in 0..start_word {
            // Step LFSR 32* (one word) without storing the output.
            for _ in 0..32 {
                prbs_state = prbs31_step(prbs_state);
            }
        }
    }

    for i in 0..words {
        let w = match pat {
            Pattern::All0 => 0x0000_0000_u32,
            Pattern::All1 => 0xFFFF_FFFF_u32,
            Pattern::Alt5555Aaaa => {
                if (start_word + i as u32) & 1 == 0 {
                    0x5555_5555
                } else {
                    0xAAAA_AAAA
                }
            }
            Pattern::Alt16 => {
                if (start_word + i as u32) & 1 == 0 {
                    0xFFFF_0000
                } else {
                    0x0000_FFFF
                }
            }
            Pattern::Walk1s => 1u32 << ((start_word + i as u32) & 31),
            Pattern::IdfRef => IDF_REF_DATA[i % IDF_REF_DATA.len()],
            Pattern::Prbs31 => prbs31_word(&mut prbs_state),
        };
        buf[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
}

// ── ROM helper bindings (linked from esp32p4.rom.ld) ──
#[repr(C)]
struct EspRomSpiCmd {
    cmd: u16,
    cmd_bit_len: u16,
    addr: *mut u32,
    addr_bit_len: u32,
    tx_data: *mut u32,
    tx_data_bit_len: u32,
    rx_data: *mut u32,
    rx_data_bit_len: u32,
    dummy_bit_len: u32,
}

const ESP_ROM_SPIFLASH_OPI_DTR_MODE: u32 = 7;
const ROM_SPI_PSRAM_CMD_NUM: i32 = 3;
const ROM_SPI_PSRAM_CS_MASK: u8 = 1 << 1;

const AP_HEX_SYNC_READ: u16 = 0x0000;
const AP_HEX_SYNC_WRITE: u16 = 0x8080;
const AP_HEX_RD_CMD_BITLEN: u16 = 16;
const AP_HEX_WR_CMD_BITLEN: u16 = 16;
const AP_HEX_ADDR_BITLEN: u32 = 32;
// Top-speed dummy bit-counts. Match `SpiRamFreq::Mhz250` in the driver
// (RL=6, WL=3 -> AP_HEX_PSRAM_RD/WR_DUMMY_BITLEN = 2*(18-1)/2*(9-1)).
// Driver default is now 250 MHz; the shmoo is intentionally
// characterising the **top-speed** corner where timing margin is
// tightest, so all SPI transactions issued from this binary use the
// 250 MHz dummy values.
const AP_HEX_RD_DUMMY_BITLEN: u32 = 34; // 2*(18-1) for SPIRAM_SPEED_250M
const AP_HEX_WR_DUMMY_BITLEN: u32 = 16; // 2*(9-1)  for SPIRAM_SPEED_250M
/// Register-read dummy at 250 MHz (`AP_HEX_PSRAM_RD_REG_DUMMY_BITLEN`).
const AP_HEX_REG_DUMMY_BITLEN: u32 = 16;

unsafe extern "C" {
    fn esp_rom_spi_set_op_mode(spi_num: i32, mode: u32);
    fn esp_rom_spi_cmd_config(spi_num: i32, pcmd: *mut EspRomSpiCmd);
}

/// Manually kick `SPI_USR` (bit 18 of CMD reg) and bounded-poll for
/// completion. Replaces ROM `esp_rom_spi_cmd_start` which polls forever.
/// Returns Ok with iters consumed, or Err on timeout.
fn mspi1_kick_and_collect(rx: &mut [u8]) -> Result<u32, u32> {
    const MAX_ITERS: u32 = 1_000_000;
    // Select CS1 (PSRAM); leave CS0 (flash) disabled.
    const MISC: u32 = MSPI1_BASE + 0x34;
    unsafe {
        let v = (MISC as *const u32).read_volatile();
        let v = (v | 0x1) & !0x2;
        (MISC as *mut u32).write_volatile(v);
        (MSPI1_CMD as *mut u32).write_volatile(SPI_USR_TRIGGER);
    }
    let mut t = 0u32;
    while unsafe { (MSPI1_CMD as *const u32).read_volatile() } & SPI_USR_TRIGGER != 0 {
        t += 1;
        if t >= MAX_ITERS {
            return Err(t);
        }
        core::hint::spin_loop();
    }
    if !rx.is_empty() {
        let n_words = (rx.len() + 3) / 4;
        for i in 0..n_words {
            let word =
                unsafe { ((MSPI1_W0 + (i as u32) * 4) as *const u32).read_volatile() };
            for b in 0..4 {
                let off = i * 4 + b;
                if off >= rx.len() {
                    break;
                }
                rx[off] = ((word >> (b * 8)) & 0xFF) as u8;
            }
        }
    }
    Ok(t)
}

/// Write up to `PSRAM_CTRLR_LL_FIFO_MAX_BYTES = 64` bytes via MSPI3
/// SYNC_WRITE (0x8080). Same flow as IDF
/// `mspi_timing_config_psram_write_data` but Rust-only.
fn mspi1_psram_write(addr: u32, buf: &[u8]) -> Result<(), u32> {
    assert!(buf.len() <= 64);
    let mut addr_local = addr;
    // Copy buf into a u32-aligned scratch we can hand to ROM.
    let mut tx_words = [0u32; 16];
    for (i, b) in buf.iter().enumerate() {
        tx_words[i / 4] |= (*b as u32) << ((i % 4) * 8);
    }
    let mut conf = EspRomSpiCmd {
        cmd: AP_HEX_SYNC_WRITE,
        cmd_bit_len: AP_HEX_WR_CMD_BITLEN,
        addr: &mut addr_local as *mut u32,
        addr_bit_len: AP_HEX_ADDR_BITLEN,
        tx_data: tx_words.as_mut_ptr(),
        tx_data_bit_len: (buf.len() * 8) as u32,
        rx_data: core::ptr::null_mut(),
        rx_data_bit_len: 0,
        dummy_bit_len: AP_HEX_WR_DUMMY_BITLEN,
    };
    unsafe {
        esp_rom_spi_set_op_mode(ROM_SPI_PSRAM_CMD_NUM, ESP_ROM_SPIFLASH_OPI_DTR_MODE);
        esp_rom_spi_cmd_config(ROM_SPI_PSRAM_CMD_NUM, &mut conf as *mut EspRomSpiCmd);
    }
    let mut empty: [u8; 0] = [];
    mspi1_kick_and_collect(&mut empty).map(|_| ())
}

/// AP HEX PSRAM register-read (cmd 0x4040). 16-bit MISO into `out`.
fn mspi1_mr_read16(addr: u32) -> u16 {
    let mut addr_local = addr;
    let mut rx: [u8; 2] = [0; 2];
    let mut conf = EspRomSpiCmd {
        cmd: 0x4040,
        cmd_bit_len: 16,
        addr: &mut addr_local as *mut u32,
        addr_bit_len: 32,
        tx_data: core::ptr::null_mut(),
        tx_data_bit_len: 0,
        rx_data: rx.as_mut_ptr() as *mut u32,
        rx_data_bit_len: 16,
        dummy_bit_len: AP_HEX_REG_DUMMY_BITLEN,
    };
    unsafe {
        esp_rom_spi_set_op_mode(ROM_SPI_PSRAM_CMD_NUM, ESP_ROM_SPIFLASH_OPI_DTR_MODE);
        esp_rom_spi_cmd_config(ROM_SPI_PSRAM_CMD_NUM, &mut conf as *mut EspRomSpiCmd);
    }
    let _ = mspi1_kick_and_collect(&mut rx);
    u16::from_le_bytes(rx)
}

/// AP HEX PSRAM register-write (cmd 0xC0C0). 16-bit MOSI from `data`.
fn mspi1_mr_write16(addr: u32, data: u16) {
    let mut addr_local = addr;
    let mut tx = data.to_le_bytes();
    let mut conf = EspRomSpiCmd {
        cmd: 0xC0C0,
        cmd_bit_len: 16,
        addr: &mut addr_local as *mut u32,
        addr_bit_len: 32,
        tx_data: tx.as_mut_ptr() as *mut u32,
        tx_data_bit_len: 16,
        rx_data: core::ptr::null_mut(),
        rx_data_bit_len: 0,
        dummy_bit_len: 0,
    };
    unsafe {
        esp_rom_spi_set_op_mode(ROM_SPI_PSRAM_CMD_NUM, ESP_ROM_SPIFLASH_OPI_DTR_MODE);
        esp_rom_spi_cmd_config(ROM_SPI_PSRAM_CMD_NUM, &mut conf as *mut EspRomSpiCmd);
    }
    let mut empty: [u8; 0] = [];
    let _ = mspi1_kick_and_collect(&mut empty);
}

/// Read MR0 (low byte of MR0+MR1 pair at addr 0x0).
fn read_mr0() -> u8 {
    (mspi1_mr_read16(0x0) & 0xFF) as u8
}

/// Write MR0, preserving MR1 byte (read-modify-write of the 16-bit pair).
fn write_mr0(new_mr0: u8) {
    let mr01 = mspi1_mr_read16(0x0);
    let new = (new_mr0 as u16) | (mr01 & 0xFF00);
    mspi1_mr_write16(0x0, new);
}

/// Set MR0.drive_str (bits[1:0]), preserving other MR0 fields.
fn set_drive_str(drv: u8) {
    debug_assert!(drv < 4);
    let mr0 = read_mr0();
    let new = (mr0 & !0x3) | (drv & 0x3);
    write_mr0(new);
}

/// Set MR0.read_latency (bits[4:2]), preserving other fields.
fn set_read_latency_mr(rl: u8) {
    debug_assert!(rl < 8);
    let mr0 = read_mr0();
    let new = (mr0 & !(0x7 << 2)) | ((rl & 0x7) << 2);
    write_mr0(new);
}

/// PMU EXT_LDO_P1_0P1A_ANA register — controls the analog supply LDO
/// for the MSPI PHY. The DREF field [31:28] sets the internal Vref of
/// the LDO; writing different DREF values re-trims the LDO output
/// voltage, which moves the absolute reference of every input sampler
/// in the MSPI PHY at once. This is the closest functional analog
/// JEDEC LPDDR4 Vref_DQ training has on this IP — there is no true
/// "Vref offset" register, but this is what corresponds to "shift the
/// analog reference voltage."
///
/// SAFETY: stay in DREF ∈ [3..7]. Default at boot is 5 (after we
/// program 0x57000000, see `psram_phy_ldo_init`). Outside that range
/// the LDO output can fall below 1.6 V or rise above 2.0 V, which is
/// outside the MSPI PHY supply window and may damage the chip.
const PMU_EXT_LDO_P1_0P1A_ANA: u32 = 0x5011_5000 + 0x1D4;

fn read_phy_ldo_ana() -> u32 {
    unsafe { (PMU_EXT_LDO_P1_0P1A_ANA as *const u32).read_volatile() }
}

fn set_phy_dref(dref: u8) {
    debug_assert!(dref < 16);
    unsafe {
        let r = PMU_EXT_LDO_P1_0P1A_ANA as *mut u32;
        let v = r.read_volatile();
        let v = (v & !(0xF << 28)) | ((dref as u32) << 28);
        r.write_volatile(v);
    }
    // Allow the analog block to settle before exercising DDR I/O.
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
}

/// Update MSPI0 controller's read-dummy cycle field to match the
/// device's read latency. Without this the controller and chip will
/// disagree on dummy cycle count and reads will be garbage.
///
/// Field: `CACHE_SCTRL.sram_rdummy_cyclelen` at MSPI0 base + 0x40
/// bits [11:6]. Value to write = `bit_count - 1` where
/// `bit_count = 2 * (rl_cycles - 1)` for AP HEX 200 MHz convention,
/// and `rl_cycles = 6 + 2 * mr0_read_latency`.
fn set_controller_rd_dummy(mr0_rl: u8) {
    let rl_cycles = 6 + 2 * (mr0_rl as u32);
    let dummy_bit_count = 2 * (rl_cycles - 1);
    let cyclelen_field = dummy_bit_count - 1;
    const SCTRL: u32 = 0x5008_E000 + 0x40;
    unsafe {
        let r = SCTRL as *mut u32;
        let v = r.read_volatile();
        let v = (v & !(0x3F << 6)) | ((cyclelen_field & 0x3F) << 6);
        r.write_volatile(v);
    }
}

/// Read up to 64 bytes via MSPI3 SYNC_READ (0x0000).
fn mspi1_psram_read(addr: u32, buf: &mut [u8]) -> Result<(), u32> {
    assert!(buf.len() <= 64);
    let mut addr_local = addr;
    let mut conf = EspRomSpiCmd {
        cmd: AP_HEX_SYNC_READ,
        cmd_bit_len: AP_HEX_RD_CMD_BITLEN,
        addr: &mut addr_local as *mut u32,
        addr_bit_len: AP_HEX_ADDR_BITLEN,
        tx_data: core::ptr::null_mut(),
        tx_data_bit_len: 0,
        rx_data: buf.as_mut_ptr() as *mut u32,
        rx_data_bit_len: (buf.len() * 8) as u32,
        dummy_bit_len: AP_HEX_RD_DUMMY_BITLEN,
    };
    unsafe {
        esp_rom_spi_set_op_mode(ROM_SPI_PSRAM_CMD_NUM, ESP_ROM_SPIFLASH_OPI_DTR_MODE);
        esp_rom_spi_cmd_config(ROM_SPI_PSRAM_CMD_NUM, &mut conf as *mut EspRomSpiCmd);
    }
    mspi1_kick_and_collect(buf).map(|_| ())
}

// ── IOMUX phase + delayline writers ──

/// Set DQS_0 + DQS_1 phase to one of the 4 supported values (0..3).
/// Bits [2:1] of the per-DQS pin register.
fn set_dqs_phase(phase: u8) {
    debug_assert!(phase < 4);
    unsafe {
        for &reg in &[PSRAM_DQS_0_REG, PSRAM_DQS_1_REG] {
            let r = reg as *mut u32;
            let v = r.read_volatile();
            // clear bits[2:1], write phase shifted left by 1
            let v = (v & !(0x3 << 1)) | ((phase as u32) << 1);
            r.write_volatile(v);
        }
    }
}

/// Set delayline value (0..15) on every PSRAM data + clk + cs pin AND
/// on both DQS strobe pins. Matches IDF's `mspi_timing_ll_set_delayline`
/// for `MSPI_LL_PIN_*` (we sweep both axes synchronized; see file
/// header for why per-pin deskew is absent).
fn set_delayline(dly: u8) {
    debug_assert!(dly < 16);
    unsafe {
        // Data pins: bits [7:4] = pin_dlc.
        for &off in DATA_PIN_OFFSETS {
            let r = (IOMUX_MSPI_BASE + off) as *mut u32;
            let v = r.read_volatile();
            let v = (v & !(0xF << 4)) | ((dly as u32) << 4);
            r.write_volatile(v);
        }
        // DQS pins: bits [10:7] = delay_90, bits [20:17] = delay_270.
        for &reg in &[PSRAM_DQS_0_REG, PSRAM_DQS_1_REG] {
            let r = reg as *mut u32;
            let v = r.read_volatile();
            let v = (v & !((0xF << 7) | (0xF << 17)))
                | ((dly as u32) << 7)
                | ((dly as u32) << 17);
            r.write_volatile(v);
        }
    }
}

/// Cache-flush helper. Even though the SPI direct path does NOT go
/// through L1/L2 D-cache, future cache-side AXI access to the same
/// PSRAM region might pick up stale lines from previous runs. After
/// each burst write we invalidate the entire cache to prevent any
/// false-positive readback from stale lines after a write storm.
unsafe extern "C" {
    fn Cache_Invalidate_All();
}

#[inline(always)]
fn cache_flush_all() {
    unsafe { Cache_Invalidate_All() };
}

/// Test one cell of the shmoo grid by writing then reading back a full
/// burst (`BURST_LEN` bytes = `BURST_CHUNKS` * `FIFO_MAX`) for **every**
/// pattern in `PATTERNS`. The cell passes only if all patterns survive
/// a write-burst -> cache-flush -> read-burst -> compare round trip.
///
/// Sequence per pattern:
///   1. Fill scratch buffer with the pattern (BURST_LEN bytes).
///   2. Burst-write to PSRAM via MSPI3 SYNC_WRITE in 64-B chunks.
///   3. `Cache_Invalidate_All` so that no later cache-side access can
///      return a stale value, AND no earlier cache prefetch can shadow
///      the newly written data on read-back through alternate paths.
///   4. Burst-read back via MSPI3 SYNC_READ.
///   5. Byte-wise compare against the written reference.
///
/// Failure on any pattern fails the cell (even if other patterns pass).
fn test_one_point() -> bool {
    let mut buf = [0u8; BURST_LEN];
    let mut got = [0u8; BURST_LEN];

    for &pat in PATTERNS {
        // Fill reference buffer with the chosen pattern.
        fill_pattern(&mut buf, pat, 0);

        // Burst-write in 64-B chunks across the BURST_LEN region.
        for chunk in 0..BURST_CHUNKS {
            let off = chunk * FIFO_MAX;
            let addr = PSRAM_TEST_ADDR + off as u32;
            if mspi1_psram_write(addr, &buf[off..off + FIFO_MAX]).is_err() {
                return false;
            }
        }

        // Drop any cached lines that might shadow the readback path.
        cache_flush_all();

        // Burst-read back.
        for chunk in 0..BURST_CHUNKS {
            let off = chunk * FIFO_MAX;
            let addr = PSRAM_TEST_ADDR + off as u32;
            if mspi1_psram_read(addr, &mut got[off..off + FIFO_MAX]).is_err() {
                return false;
            }
        }

        // Compare the entire burst (any single byte mismatch fails).
        if got != buf {
            return false;
        }
    }
    true
}

const PHASE_DEG: [&str; 4] = [" 67.5°", "78.75°", " 90.0°", "101.25°"];

/// Pick best phase = end of longest passing run, matching IDF
/// `mspi_timing_psram_select_best_tuning_phase`.
fn select_best_phase(grid: &[[bool; 16]; 4]) -> Option<u8> {
    let mut best_phase = None;
    let mut best_len = 0u32;
    for (id, row) in grid.iter().enumerate() {
        // count any pass in the row
        let pass_count = row.iter().filter(|&&p| p).count() as u32;
        if pass_count > best_len {
            best_len = pass_count;
            best_phase = Some(id as u8);
        }
    }
    best_phase
}

/// Pick best delayline = center of longest passing run on the chosen
/// phase row, matching IDF `mspi_timing_psram_select_best_tuning_delayline`.
fn select_best_delayline(row: &[bool; 16]) -> Option<u8> {
    let mut longest = 0usize;
    let mut longest_end = 0usize;
    let mut cur = 0usize;
    let mut cur_end = 0usize;
    for (i, &p) in row.iter().enumerate() {
        if p {
            cur += 1;
            cur_end = i;
            if cur > longest {
                longest = cur;
                longest_end = cur_end;
            }
        } else {
            cur = 0;
        }
    }
    if longest <= 1 {
        None
    } else {
        Some((longest_end - longest / 2) as u8)
    }
}

/// Print a 4-row * 16-col shmoo grid with the given y-axis labels and
/// header. Used for both Grid 1 (phase) and Grid 2 (drive_str).
fn print_grid(header: &str, ylabels: &[&str; 4], grid: &[[bool; 16]; 4]) {
    info!("");
    info!("  {:20}  0 1 2 3 4 5 6 7 8 9 a b c d e f", header);
    info!("  ──────────────────────  ─────────────────────────────────");
    for (id, row) in grid.iter().enumerate() {
        let mut buf: heapless::String<128> = heapless::String::new();
        let _ = write!(buf, "  {:>16} ({})    ", ylabels[id], id);
        for &p in row.iter() {
            let _ = buf.push(if p { 'P' } else { '.' });
            let _ = buf.push(' ');
        }
        info!("{}", buf.as_str());
    }
    info!("");
}

/// Like `print_grid` but with N rows (used for read_latency = 6 rows).
fn print_grid_n<const N: usize>(header: &str, ylabels: &[&str; N], grid: &[[bool; 16]; N]) {
    info!("");
    info!("  {:20}  0 1 2 3 4 5 6 7 8 9 a b c d e f", header);
    info!("  ──────────────────────  ─────────────────────────────────");
    for (id, row) in grid.iter().enumerate() {
        let mut buf: heapless::String<128> = heapless::String::new();
        let _ = write!(buf, "  {:>16} ({})    ", ylabels[id], id);
        for &p in row.iter() {
            let _ = buf.push(if p { 'P' } else { '.' });
            let _ = buf.push(' ');
        }
        info!("{}", buf.as_str());
    }
    info!("");
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_psram_shmoo -- DQS phase x delayline sweep");
    info!("================================================");

    info!("[1/8] Init PSRAM (Phase A..D + MMU map)...");
    let cfg = PsramConfig {
        size: PsramSize::AutoDetect,
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

    // Save the device-side defaults so we can restore between grids.
    let default_mr0 = read_mr0();
    info!(
        "  initial MR0 = 0x{:02x}  (drive_str={}, read_latency={}, lt={})",
        default_mr0,
        default_mr0 & 0x3,
        (default_mr0 >> 2) & 0x7,
        (default_mr0 >> 5) & 0x1
    );

    // ── Grid 1: phase * delayline (default drive_str + RL) ──
    info!("[2/8] Grid 1: DQS phase * delayline (4 * 16 = 64 points)");
    let mut grid1: [[bool; 16]; 4] = [[false; 16]; 4];
    for ph in 0..4u8 {
        set_dqs_phase(ph);
        for dly in 0..16u8 {
            set_delayline(dly);
            for _ in 0..200 {
                core::hint::spin_loop();
            }
            grid1[ph as usize][dly as usize] = test_one_point();
        }
    }
    print_grid("phase \\ delayline", &PHASE_DEG, &grid1);

    let pass_count1: u32 = grid1.iter().flat_map(|r| r.iter()).map(|&p| p as u32).sum();
    info!("  Grid 1: {}/64 points pass", pass_count1);

    // ── Grid 2: drive_str * delayline (phase = 90°, default RL) ──
    info!("[3/8] Grid 2: drive_str * delayline (phase=90°, RL=default)");
    set_dqs_phase(2); // 90° fixed
    let mut grid2: [[bool; 16]; 4] = [[false; 16]; 4];
    for drv in 0..4u8 {
        set_drive_str(drv);
        for dly in 0..16u8 {
            set_delayline(dly);
            for _ in 0..200 {
                core::hint::spin_loop();
            }
            grid2[drv as usize][dly as usize] = test_one_point();
        }
    }
    const DRV_LABELS: [&str; 4] = [" 25Ω", " 50Ω", "100Ω", "200Ω"];
    print_grid("drive_str \\ delayline", &DRV_LABELS, &grid2);
    let pass_count2: u32 = grid2.iter().flat_map(|r| r.iter()).map(|&p| p as u32).sum();
    info!("  Grid 2: {}/64 points pass", pass_count2);

    // Restore default drive_str before next grid.
    write_mr0(default_mr0);

    // ── Grid 3: read_latency * delayline (phase = 90°, drive_str=default) ──
    //
    // For each MR0.read_latency value we ALSO update the controller's
    // sram_rdummy_cyclelen so SoC and chip stay consistent. Without that
    // the read returns garbage regardless of timing — every cell would
    // fail on chip/SoC dummy mismatch, not on signal integrity.
    info!("[4/8] Grid 3: read_latency * delayline (phase=90°, drv=default)");
    let mut grid3: [[bool; 16]; 6] = [[false; 16]; 6];
    let rl_values: [u8; 6] = [2, 3, 4, 5, 6, 7];
    for (rl_idx, &rl) in rl_values.iter().enumerate() {
        set_read_latency_mr(rl);
        set_controller_rd_dummy(rl);
        for dly in 0..16u8 {
            set_delayline(dly);
            for _ in 0..200 {
                core::hint::spin_loop();
            }
            grid3[rl_idx][dly as usize] = test_one_point();
        }
    }
    // Cycle count formula matches IDF s_print_psram_info: cycles = 2*RL + 6.
    // So RL=2 → 10 cycles, RL=4 → 14 cycles (200 MHz default), RL=7 → 20 cycles.
    const RL_LABELS: [&str; 6] = [
        "RL=2 (10cyc)",
        "RL=3 (12cyc)",
        "RL=4 (14cyc)",
        "RL=5 (16cyc)",
        "RL=6 (18cyc)",
        "RL=7 (20cyc)",
    ];
    print_grid_n("read_latency \\ delayline", &RL_LABELS, &grid3);
    let pass_count3: u32 = grid3.iter().flat_map(|r| r.iter()).map(|&p| p as u32).sum();
    info!("  Grid 3: {}/96 points pass", pass_count3);

    // Restore default RL + controller dummy before Grid 4.
    write_mr0(default_mr0);
    set_controller_rd_dummy((default_mr0 >> 2) & 0x7);

    // ── Grid 4: FULL 4D sweep ──
    //
    //   drv (4) * RL (6) * phase (4) * delayline (16) = 1536 points.
    //
    // For each (drv, RL) slice we sweep all (phase * delayline) and
    // print only a one-line summary if 64/64 pass. If any point in the
    // slice fails we ALSO dump the 4*16 sub-grid to make the failing
    // pattern visible. This way the common "everything passes" case
    // produces a tight 24-line summary, while regressions surface
    // immediately with full diagnostics.
    info!("[5/8] Grid 4: full 4D sweep — drv * RL * phase * delayline = 1536 points");
    let mut grid4_pass = 0u32;
    let mut grid4_total = 0u32;
    for drv in 0..4u8 {
        set_drive_str(drv);
        for &rl in rl_values.iter() {
            set_read_latency_mr(rl);
            set_controller_rd_dummy(rl);
            let mut sub: [[bool; 16]; 4] = [[false; 16]; 4];
            let mut sub_pass = 0u32;
            for ph in 0..4u8 {
                set_dqs_phase(ph);
                for dly in 0..16u8 {
                    set_delayline(dly);
                    for _ in 0..200 {
                        core::hint::spin_loop();
                    }
                    let p = test_one_point();
                    sub[ph as usize][dly as usize] = p;
                    if p {
                        sub_pass += 1;
                    }
                }
            }
            grid4_pass += sub_pass;
            grid4_total += 64;
            if sub_pass == 64 {
                info!("  drv={} RL={} ({}cyc): 64/64", DRV_LABELS[drv as usize].trim(), rl, 6 + 2 * rl);
            } else {
                info!(
                    "  drv={} RL={} ({}cyc): {}/64  ── FAIL pattern below ──",
                    DRV_LABELS[drv as usize].trim(),
                    rl,
                    6 + 2 * rl,
                    sub_pass
                );
                print_grid("  phase \\ delayline", &PHASE_DEG, &sub);
            }
        }
    }
    info!("  Grid 4 total: {}/{}", grid4_pass, grid4_total);

    // Restore defaults after Grid 4.
    write_mr0(default_mr0);
    set_controller_rd_dummy((default_mr0 >> 2) & 0x7);

    // ── Grid 5: pseudo-Vref (PHY LDO DREF) * delayline ──
    //
    // The closest analog to JEDEC LPDDR4 Vref_DQ training on this IP.
    // We sweep the PMU EXT_LDO_P1_0P1A_ANA DREF field, which retrims
    // the analog supply LDO that powers the entire MSPI PHY. This
    // shifts the input sampler reference voltage and the output
    // driver swing simultaneously (it's a global supply trim, not a
    // pure Vref). Useful as a sanity check that "varying analog
    // reference voltage doesn't break us within the safe window."
    //
    // Sweep range DREF ∈ [3..7]. Outside this window the LDO output
    // can fall below 1.6 V or rise above 2.0 V which is outside the
    // MSPI PHY supply spec and may damage the chip. Do NOT widen.
    info!("[6/8] Grid 5: pseudo-Vref (PHY LDO DREF) * delayline (phase=90°)");
    let saved_ana = read_phy_ldo_ana();
    let saved_dref = ((saved_ana >> 28) & 0xF) as u8;
    info!(
        "  saved EXT_LDO_P1_0P1A_ANA = 0x{:08X}  (DREF={}, MUL={})",
        saved_ana,
        saved_dref,
        ((saved_ana >> 23) & 0x7),
    );

    set_dqs_phase(2); // 90° fixed
    let dref_values: [u8; 5] = [3, 4, 5, 6, 7];
    let mut grid5: [[bool; 16]; 5] = [[false; 16]; 5];
    for (di, &dref) in dref_values.iter().enumerate() {
        set_phy_dref(dref);
        for dly in 0..16u8 {
            set_delayline(dly);
            for _ in 0..200 {
                core::hint::spin_loop();
            }
            grid5[di][dly as usize] = test_one_point();
        }
    }
    const DREF_LABELS: [&str; 5] = [
        "DREF=3 (~1.65V)",
        "DREF=4 (~1.75V)",
        "DREF=5 (~1.88V) ←saved",
        "DREF=6 (~2.00V)",
        "DREF=7 (~2.12V)",
    ];
    print_grid_n("LDO DREF \\ delayline", &DREF_LABELS, &grid5);
    let pass_count5: u32 = grid5.iter().flat_map(|r| r.iter()).map(|&p| p as u32).sum();
    info!("  Grid 5: {}/80 points pass", pass_count5);

    // Restore the saved DREF/MUL exactly to avoid leaving the PHY in
    // a non-default voltage when the test exits.
    unsafe {
        (PMU_EXT_LDO_P1_0P1A_ANA as *mut u32).write_volatile(saved_ana);
    }

    // ── Pick best from Grid 1 (the canonical one) ──
    info!("[7/8] Best timing from Grid 1");
    let best_phase = select_best_phase(&grid1);
    let best_delayline = best_phase.and_then(|ph| select_best_delayline(&grid1[ph as usize]));
    info!(
        "  best: phase = {}, delayline = {}",
        best_phase
            .map(|ph| PHASE_DEG[ph as usize])
            .unwrap_or("(none)"),
        best_delayline
            .map(|d| {
                let mut s: heapless::String<8> = heapless::String::new();
                let _ = write!(s, "{}", d);
                s
            })
            .unwrap_or_else(|| {
                let mut s: heapless::String<8> = heapless::String::new();
                let _ = s.push_str("(none)");
                s
            })
            .as_str()
    );

    info!("[8/8] Apply best, attempt cache-side AXI access at 0x4800_0000");
    if let (Some(ph), Some(dly)) = (best_phase, best_delayline) {
        set_dqs_phase(ph);
        set_delayline(dly);
        // Invalidate cache so any stale lines from earlier failed attempts
        // are flushed before exercising the cache-side AXI path.
        cache_flush_all();

        let probe = 0xCAFE_BABE_u32;
        let vaddr = 0x4800_0000_usize as *mut u32;
        let read_back: u32;
        unsafe {
            vaddr.write_volatile(probe);
            read_back = vaddr.read_volatile();
        }
        if read_back == probe {
            info!("  AXI access OK: wrote 0x{:08X}, read 0x{:08X}", probe, read_back);
            info!("  test_psram_shmoo: PASS");
            esp32p4_hal_testing::signal_pass();
        } else {
            info!(
                "  AXI access mismatch: wrote 0x{:08X}, read 0x{:08X}",
                probe, read_back
            );
            info!("  test_psram_shmoo: FAIL (timing tuned but cache fill still wrong)");
        }
    } else {
        info!("  no passing point in shmoo grid -- chip not responding, or PHY not initialized");
        info!("  test_psram_shmoo: FAIL");
    }

    info!("================================================");
    esp32p4_hal_testing::park_alive("test_psram_shmoo");
}
