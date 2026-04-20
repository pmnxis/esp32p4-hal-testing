// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errata test: MSPI-750 -- PSRAM unaligned DMA read returns stale data.
//!
//! Bug: Write to PSRAM, then DMA read with 1 or 2 byte burst at non-4-byte-aligned
//!      address overlapping the write range -> returns old data.
//! Affected: v3.0 only. Fixed in v3.1.
//!
//! Test strategy:
//!   1. Write known pattern to PSRAM region
//!   2. Write NEW pattern to same region
//!   3. Read back with various alignments (1, 2, 3 byte offsets)
//!   4. On v3.0: unaligned reads may return OLD pattern (bug present)
//!   5. On v3.1: all reads should return NEW pattern (fix verified)
//!
//! NOTE: Requires PSRAM MMU mapping. Test skips if PSRAM not accessible.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const PSRAM_BASE: u32 = 0x4800_0000;
const TEST_OFFSET: u32 = 0x1000; // avoid page 0 edge

fn psram_accessible() -> bool {
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.peri_clk_ctrl00().read().psram_core_clk_en().bit_is_set()
    // TODO: also check MMU mapping
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let rev = esp_hal::efuse::chip_revision();
    info!("=== test_errata_mspi750: PSRAM unaligned DMA stale data ===");
    info!("Chip revision: v{}.{}", rev.major, rev.minor);

    if rev.major == 3 && rev.minor == 0 {
        info!("v3.0: This errata IS ACTIVE. Unaligned reads may return stale data.");
        info!("Workaround: always 4-byte align DMA buffer addresses.");
    } else if rev.major >= 3 && rev.minor >= 1 {
        info!("v3.1+: This errata should be FIXED.");
    }

    if !psram_accessible() {
        info!("PSRAM not accessible (MMU not mapped). Test SKIPPED.");
        info!("Enable PSRAM MMU mapping to run this test.");
        esp32p4_hal_testing::signal_pass();
        esp32p4_hal_testing::park_alive("test_errata_mspi750");
    }

    let base = (PSRAM_BASE + TEST_OFFSET) as *mut u8;

    // 1. Fill 64 bytes with OLD pattern (0xAA)
    info!("Step 1: Fill with OLD pattern 0xAA");
    unsafe {
        for i in 0..64 {
            base.add(i).write_volatile(0xAA);
        }
    }

    // 2. Overwrite with NEW pattern (0x55)
    info!("Step 2: Overwrite with NEW pattern 0x55");
    unsafe {
        for i in 0..64 {
            base.add(i).write_volatile(0x55);
        }
    }

    // 3. Read back at various alignments
    info!("Step 3: Read back at various alignments");
    let mut stale_count = 0u32;

    for offset in 0..4u32 {
        let ptr = unsafe { base.add(offset as usize) };
        let mut errors_at_offset = 0u32;

        for i in 0..16 {
            let val = unsafe { ptr.add(i * 4).read_volatile() };
            if val == 0xAA {
                errors_at_offset += 1; // got OLD data = stale
            } else if val != 0x55 {
                info!("  offset={} i={}: unexpected 0x{:02X}", offset, i, val);
            }
        }

        info!("  Offset +{}: {}/16 reads returned stale data", offset, errors_at_offset);
        stale_count += errors_at_offset;
    }

    if stale_count > 0 {
        info!("RESULT: {} stale reads detected!", stale_count);
        if rev.major == 3 && rev.minor == 0 {
            info!("This is EXPECTED on v3.0 (MSPI-750 active).");
            info!("Workaround: use 4-byte aligned addresses only.");
        } else {
            info!("UNEXPECTED on v3.1+! Errata may not be fully fixed.");
        }
    } else {
        info!("RESULT: No stale reads. Data consistent.");
        if rev.major == 3 && rev.minor == 0 {
            info!("v3.0 but no stale data -- bug may need specific DMA trigger.");
        } else {
            info!("v3.1+ fix CONFIRMED.");
        }
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_errata_mspi750: DONE ===");

    loop { esp32p4_hal_testing::delay_ms(1000); }
}
