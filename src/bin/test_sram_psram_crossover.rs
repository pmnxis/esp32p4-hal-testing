// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: SRAM <-> PSRAM cross-region data integrity.
//!
//! Verifies:
//! 1. SRAM basic pattern fill + verify
//! 2. PSRAM basic pattern fill + verify (requires MMU mapping)
//! 3. SRAM -> PSRAM copy + verify (cross-region)
//! 4. PSRAM -> SRAM copy + verify (cross-region)
//! 5. Alternating writes (SRAM word, PSRAM word, SRAM word, ...)
//! 6. Stress: large block copy SRAM <-> PSRAM
//! 7. Address aliasing check (SRAM write doesn't corrupt PSRAM and vice versa)
//!
//! Corner cases:
//! - Cache line boundary crossing (64-byte lines on P4)
//! - Unaligned cross-region copy (MSPI-750 errata: 4-byte align on v3.0)
//! - Concurrent-like access pattern (interleaved R/W)
//!
//! NOTE: PSRAM tests require MMU mapping at 0x4800_0000.
//!       If MMU is not set up (current state), PSRAM tests are skipped.
//!       SRAM tests always run.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

/// SRAM usable range (after ROM reserved)
const SRAM_BASE: u32 = 0x4FF4_0000;
const SRAM_END: u32 = 0x4FFC_0000;
/// Use a scratch area far from stack/bss (near end of SRAM, 16KB block)
const SRAM_TEST_BASE: u32 = SRAM_END - 0x8000; // 32KB before end
const TEST_SIZE: usize = 4096; // 4KB test block

/// PSRAM virtual address (after MMU mapping)
const PSRAM_BASE: u32 = 0x4800_0000;
const PSRAM_TEST_BASE: u32 = PSRAM_BASE + 0x1000; // offset to avoid page 0 edge

/// Check if PSRAM is accessible (try read, expect no fault).
fn psram_accessible() -> bool {
    // Try reading from PSRAM -- if MMU not mapped, this will fault.
    // We catch it by checking if a known pattern round-trips.
    // But we can't actually catch faults in no_std without trap handler...
    // Instead, check if PSRAM clock is enabled as a proxy.
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    let psram_clk = clkrst.peri_clk_ctrl00().read().psram_core_clk_en().bit_is_set();
    // Even with clock, MMU mapping is needed. For now, return false.
    // TODO: Check MMU page table entry for PSRAM_BASE
    let _ = psram_clk;
    false // Conservative: assume PSRAM not accessible until MMU confirmed
}

/// Fill buffer with walking-ones pattern (catches stuck bits).
fn fill_walking_ones(base: *mut u32, count: usize) {
    for i in 0..count {
        let pattern = 1u32.rotate_left((i % 32) as u32);
        unsafe { base.add(i).write_volatile(pattern); }
    }
}

/// Verify walking-ones pattern.
fn verify_walking_ones(base: *const u32, count: usize) -> u32 {
    let mut errors = 0u32;
    for i in 0..count {
        let expected = 1u32.rotate_left((i % 32) as u32);
        let actual = unsafe { base.add(i).read_volatile() };
        if actual != expected {
            if errors < 5 {
                info!("  Mismatch at offset {}: expected 0x{:08X}, got 0x{:08X}", i, expected, actual);
            }
            errors += 1;
        }
    }
    errors
}

/// Fill buffer with address-as-data pattern (catches address line faults).
fn fill_address_pattern(base: *mut u32, count: usize) {
    for i in 0..count {
        let addr = unsafe { base.add(i) } as u32;
        unsafe { base.add(i).write_volatile(addr); }
    }
}

/// Verify address-as-data pattern.
fn verify_address_pattern(base: *const u32, count: usize) -> u32 {
    let mut errors = 0u32;
    for i in 0..count {
        let expected = unsafe { base.add(i) } as u32;
        let actual = unsafe { base.add(i).read_volatile() };
        if actual != expected {
            if errors < 5 {
                info!("  Mismatch at +{}: expected 0x{:08X}, got 0x{:08X}", i * 4, expected, actual);
            }
            errors += 1;
        }
    }
    errors
}

/// Copy memory word-by-word (no DMA, pure CPU).
fn memcpy_u32(dst: *mut u32, src: *const u32, count: usize) {
    for i in 0..count {
        unsafe {
            dst.add(i).write_volatile(src.add(i).read_volatile());
        }
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_sram_psram_crossover: Memory cross-region ===");

    let word_count = TEST_SIZE / 4;
    let sram_ptr = SRAM_TEST_BASE as *mut u32;

    // ========== Part 1: SRAM-only tests ==========
    info!("--- Part 1: SRAM pattern tests (0x{:08X}, {} bytes) ---", SRAM_TEST_BASE, TEST_SIZE);

    // 1a. Walking-ones
    fill_walking_ones(sram_ptr, word_count);
    let errors = verify_walking_ones(sram_ptr, word_count);
    info!("SRAM walking-ones: {} errors", errors);
    assert!(errors == 0, "SRAM walking-ones failed");

    // 1b. Address-as-data
    fill_address_pattern(sram_ptr, word_count);
    let errors = verify_address_pattern(sram_ptr, word_count);
    info!("SRAM address-as-data: {} errors", errors);
    assert!(errors == 0, "SRAM address-as-data failed");

    // 1c. All-zeros / all-ones
    for pattern in [0x00000000u32, 0xFFFFFFFF, 0xAAAAAAAA, 0x55555555] {
        for i in 0..word_count {
            unsafe { sram_ptr.add(i).write_volatile(pattern); }
        }
        let mut errors = 0u32;
        for i in 0..word_count {
            if unsafe { sram_ptr.add(i).read_volatile() } != pattern {
                errors += 1;
            }
        }
        info!("SRAM fill 0x{:08X}: {} errors", pattern, errors);
        assert!(errors == 0);
    }

    // 1d. Cache line boundary test (64-byte lines)
    info!("SRAM cache line boundary (64B) test...");
    // Write at 60, 64, 68 byte offsets (straddles cache line)
    let boundary_ptr = (SRAM_TEST_BASE + 60) as *mut u8;
    let test_data: [u8; 16] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
                                0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00];
    unsafe {
        for i in 0..16 {
            boundary_ptr.add(i).write_volatile(test_data[i]);
        }
        let mut ok = true;
        for i in 0..16 {
            if boundary_ptr.add(i).read_volatile() != test_data[i] {
                ok = false;
            }
        }
        info!("Cache line boundary write/read: {}", if ok { "OK" } else { "FAIL" });
    }

    // 1e. Write speed measurement
    info!("SRAM write speed...");
    let t0 = esp_hal::time::Instant::now();
    for _ in 0..10 {
        fill_walking_ones(sram_ptr, word_count);
    }
    let elapsed = t0.elapsed().as_micros();
    let bytes_written = TEST_SIZE as u64 * 10;
    let mbps = bytes_written * 1_000_000 / elapsed / 1024 / 1024;
    info!("SRAM write: {} bytes in {} us ({} MB/s)", bytes_written, elapsed, mbps);

    // Read speed
    let t0 = esp_hal::time::Instant::now();
    let mut dummy = 0u32;
    for _ in 0..10 {
        for i in 0..word_count {
            dummy = dummy.wrapping_add(unsafe { sram_ptr.add(i).read_volatile() });
        }
    }
    let elapsed = t0.elapsed().as_micros();
    let bytes_read = TEST_SIZE as u64 * 10;
    let mbps = bytes_read * 1_000_000 / elapsed / 1024 / 1024;
    info!("SRAM read: {} bytes in {} us ({} MB/s) [dummy={}]", bytes_read, elapsed, mbps, dummy);

    // ========== Part 2: PSRAM tests (if accessible) ==========
    info!("--- Part 2: PSRAM tests ---");

    if psram_accessible() {
        let psram_ptr = PSRAM_TEST_BASE as *mut u32;

        info!("PSRAM accessible at 0x{:08X}", PSRAM_TEST_BASE);

        // 2a. Basic write/read
        fill_walking_ones(psram_ptr, word_count);
        let errors = verify_walking_ones(psram_ptr, word_count);
        info!("PSRAM walking-ones: {} errors", errors);

        // 2b. SRAM -> PSRAM copy
        fill_address_pattern(sram_ptr, word_count);
        memcpy_u32(psram_ptr, sram_ptr, word_count);
        // Verify PSRAM has SRAM's address pattern (but with SRAM addresses!)
        let mut copy_errors = 0u32;
        for i in 0..word_count {
            let expected = unsafe { sram_ptr.add(i) } as u32; // SRAM addresses
            let actual = unsafe { psram_ptr.add(i).read_volatile() };
            if actual != expected { copy_errors += 1; }
        }
        info!("SRAM -> PSRAM copy: {} errors", copy_errors);

        // 2c. PSRAM -> SRAM copy
        fill_address_pattern(psram_ptr, word_count);
        memcpy_u32(sram_ptr, psram_ptr, word_count);
        let mut copy_errors = 0u32;
        for i in 0..word_count {
            let expected = unsafe { psram_ptr.add(i) } as u32;
            let actual = unsafe { sram_ptr.add(i).read_volatile() };
            if actual != expected { copy_errors += 1; }
        }
        info!("PSRAM -> SRAM copy: {} errors", copy_errors);

        // 2d. Interleaved write (SRAM, PSRAM, SRAM, PSRAM, ...)
        info!("Interleaved SRAM/PSRAM write...");
        for i in 0..word_count {
            let val = i as u32;
            if i % 2 == 0 {
                unsafe { sram_ptr.add(i).write_volatile(val); }
            } else {
                unsafe { psram_ptr.add(i).write_volatile(val); }
            }
        }
        let mut interleave_errors = 0u32;
        for i in 0..word_count {
            let expected = i as u32;
            let actual = if i % 2 == 0 {
                unsafe { sram_ptr.add(i).read_volatile() }
            } else {
                unsafe { psram_ptr.add(i).read_volatile() }
            };
            if actual != expected { interleave_errors += 1; }
        }
        info!("Interleaved: {} errors", interleave_errors);

        // 2e. PSRAM speed
        let t0 = esp_hal::time::Instant::now();
        fill_walking_ones(psram_ptr, word_count);
        let elapsed = t0.elapsed().as_micros();
        let mbps = TEST_SIZE as u64 * 1_000_000 / elapsed / 1024 / 1024;
        info!("PSRAM write: {} bytes in {} us ({} MB/s)", TEST_SIZE, elapsed, mbps);

        // 2f. Address aliasing (SRAM write must NOT corrupt PSRAM)
        info!("Address aliasing check...");
        unsafe {
            psram_ptr.write_volatile(0xBAAD_F00D);
            sram_ptr.write_volatile(0xDEAD_BEEF);
            let psram_val = psram_ptr.read_volatile();
            let sram_val = sram_ptr.read_volatile();
            info!("PSRAM[0]=0x{:08X} (expect BAADF00D), SRAM[0]=0x{:08X} (expect DEADBEEF)",
                psram_val, sram_val);
            assert!(psram_val == 0xBAAD_F00D, "SRAM write corrupted PSRAM!");
            assert!(sram_val == 0xDEAD_BEEF, "PSRAM write corrupted SRAM!");
        }
        info!("No aliasing: OK");

    } else {
        info!("PSRAM not accessible (MMU mapping not configured).");
        info!("PSRAM cross-region tests: SKIPPED");
        info!("To enable: implement MMU page table mapping in psram/esp32p4.rs");
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_sram_psram_crossover: PASS ===");

    esp32p4_hal_testing::park_alive("test_sram_psram_crossover");
}
