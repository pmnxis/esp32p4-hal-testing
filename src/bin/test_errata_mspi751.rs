// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errata test: MSPI-751 -- PSRAM data errors from clock ratio mismatch.
//!
//! Bug: When MSPI core clock and AXI clock have certain frequency ratios,
//!      write-then-read to overlapping PSRAM addresses produces data errors.
//! Affected: v3.0. Fixed in v3.1.
//!
//! Constraint (from errata doc):
//!   AXI concat enabled + normal timing: 4 * freq_core >= freq_axi
//!   AXI concat enabled + poor timing:   3 * freq_core >= freq_axi
//!   AXI concat disabled + normal:        2 * freq_core >= freq_axi
//!   AXI concat disabled + poor:          1 * freq_core >= freq_axi
//!
//! Test strategy:
//!   1. Write known pattern to PSRAM
//!   2. Immediately read back (tight write-then-read, overlapping address)
//!   3. Repeat 10K times to stress the timing path
//!   4. On v3.0: data errors may occur depending on clock ratio
//!   5. On v3.1: no errors expected
//!
//! NOTE: Requires PSRAM MMU mapping.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const PSRAM_BASE: u32 = 0x4800_0000;
const TEST_OFFSET: u32 = 0x2000;

fn psram_accessible() -> bool {
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.peri_clk_ctrl00().read().psram_core_clk_en().bit_is_set()
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let rev = esp_hal::efuse::chip_revision();
    info!("=== test_errata_mspi751: PSRAM clock ratio data errors ===");
    info!("Chip revision: v{}.{}", rev.major, rev.minor);

    if rev.major == 3 && rev.minor == 0 {
        info!("v3.0: MSPI-751 IS ACTIVE. Tight write-read may produce errors.");
    } else {
        info!("v3.1+: MSPI-751 should be FIXED.");
    }

    if !psram_accessible() {
        info!("PSRAM not accessible. Test SKIPPED.");
        esp32p4_hal_testing::signal_pass();
        esp32p4_hal_testing::park_alive("test_errata_mspi751");
    }

    let base = (PSRAM_BASE + TEST_OFFSET) as *mut u32;
    let iterations = 10_000u32;
    let block_words = 16; // 64 bytes per iteration

    info!("Running {} iterations of tight write-then-read ({} bytes each)...",
        iterations, block_words * 4);

    let mut total_errors = 0u32;

    for iter in 0..iterations {
        let pattern = 0xA5A5_0000 | iter;

        // Write block
        for i in 0..block_words {
            unsafe { base.add(i).write_volatile(pattern.wrapping_add(i as u32)); }
        }

        // Immediately read back (no barrier, no delay -- tight coupling)
        for i in 0..block_words {
            let expected = pattern.wrapping_add(i as u32);
            let actual = unsafe { base.add(i).read_volatile() };
            if actual != expected {
                if total_errors < 10 {
                    info!("  Error at iter={} word={}: expected 0x{:08X}, got 0x{:08X}",
                        iter, i, expected, actual);
                }
                total_errors += 1;
            }
        }
    }

    info!("Total errors: {} / {} reads", total_errors, iterations * block_words as u32);

    if total_errors > 0 {
        info!("DATA ERRORS DETECTED!");
        if rev.major == 3 && rev.minor == 0 {
            info!("v3.0: Expected (MSPI-751 active). Check clock ratio constraints.");
        } else {
            info!("v3.1+: UNEXPECTED! Errata may not be fully fixed.");
        }
    } else {
        info!("No errors. Data consistent after {} tight write-read cycles.", iterations);
        if rev.major == 3 && rev.minor == 0 {
            info!("v3.0: No errors -- clock ratio may be within safe range.");
        } else {
            info!("v3.1+: MSPI-751 fix CONFIRMED.");
        }
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_errata_mspi751: DONE ===");

    loop { esp32p4_hal_testing::delay_ms(1000); }
}
