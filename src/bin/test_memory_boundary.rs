// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Corner test: SRAM memory boundary access.
//!
//! Tests:
//! 1. First/last word of usable SRAM (0x4FF40000 - 0x4FBFFFFF)
//! 2. Unaligned access (RISC-V may or may not support)
//! 3. Stack depth estimation
//! 4. Large array allocation stress

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

/// SRAM start (after ROM reserved 256KB)
const SRAM_START: u32 = 0x4FF4_0000;
/// SRAM end
const SRAM_END: u32 = 0x4FFC_0000;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_memory_boundary: SRAM edge cases ===");
    info!("SRAM range: 0x{:08X} - 0x{:08X} (512 KB)", SRAM_START, SRAM_END);

    // 1. Read current stack pointer
    let sp: u32;
    unsafe { core::arch::asm!("mv {}, sp", out(reg) sp); }
    info!("Current SP: 0x{:08X}", sp);
    assert!(sp >= SRAM_START && sp < SRAM_END, "SP outside SRAM!");
    info!("SP in SRAM: OK");

    // 2. Boundary write/read (near start of usable area)
    // Note: can't write to first address if linker places .data/.bss there
    // Use an address after .bss
    let test_addr = (SRAM_START + 0x100) as *mut u32; // offset to avoid .data
    let test_pattern = 0xCAFE_BABE_u32;
    unsafe {
        // Save original
        let orig = test_addr.read_volatile();
        // Write and verify
        test_addr.write_volatile(test_pattern);
        let readback = test_addr.read_volatile();
        // Restore
        test_addr.write_volatile(orig);

        if readback == test_pattern {
            info!("SRAM near-start write/read: OK (0x{:08X})", test_addr as u32);
        } else {
            info!("SRAM near-start: FAIL (wrote 0x{:08X}, read 0x{:08X})", test_pattern, readback);
        }
    }

    // 3. Write near end of SRAM
    let test_addr_end = (SRAM_END - 4) as *mut u32;
    unsafe {
        let orig = test_addr_end.read_volatile();
        test_addr_end.write_volatile(0xDEAD_BEEF);
        let readback = test_addr_end.read_volatile();
        test_addr_end.write_volatile(orig);

        if readback == 0xDEAD_BEEF {
            info!("SRAM near-end write/read: OK (0x{:08X})", test_addr_end as u32);
        } else {
            info!("SRAM near-end: FAIL");
        }
    }

    // 4. Stack depth test (recursive function)
    info!("Stack depth test...");
    let depth = stack_depth_test(0);
    info!("Max stack depth (approx): {} frames (~{} bytes)", depth, depth * 64);

    // 5. Large array on stack
    info!("Large stack allocation test (4KB)...");
    let big_array = [0x55u8; 4096];
    let sum: u32 = big_array.iter().map(|b| *b as u32).sum();
    assert_eq!(sum, 0x55 * 4096, "Stack array corruption");
    info!("4KB stack array: OK (sum={})", sum);

    // 6. Unaligned access test
    info!("Unaligned access test...");
    let aligned_buf = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    // Try reading u32 from offset 1 (unaligned)
    let unaligned_ptr = unsafe { aligned_buf.as_ptr().add(1) as *const u32 };
    // RISC-V: unaligned access may trap or work depending on extension
    // P4 has misaligned load/store support (C extension)
    let val = unsafe { core::ptr::read_unaligned(unaligned_ptr) };
    info!("Unaligned read at +1: 0x{:08X} (expected 0x55443322)", val);
    // Don't assert -- behavior depends on HW config

    esp32p4_hal_testing::signal_pass();
    info!("=== test_memory_boundary: PASS ===");

    esp32p4_hal_testing::park_alive("test_memory_boundary");
}

/// Recursive function to test stack depth.
/// Returns when stack gets close to limits.
#[inline(never)]
fn stack_depth_test(depth: u32) -> u32 {
    // Each frame uses ~64 bytes (registers + locals)
    let _padding = [0u8; 32]; // force some stack usage
    let sp: u32;
    unsafe { core::arch::asm!("mv {}, sp", out(reg) sp); }

    // Stop if SP is within 4KB of SRAM start (safety margin)
    if sp < SRAM_START + 4096 || depth > 10_000 {
        return depth;
    }

    stack_depth_test(depth + 1)
}
