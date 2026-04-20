// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: Watchdog timer operation.
//!
//! Verifies: LP_WDT can be enabled, fed, and will reset if not fed.
//! Software-only.
//!
//! Test sequence:
//! 1. Verify WDTs are disabled (by esp-hal init)
//! 2. Enable LP_WDT with 2-second timeout
//! 3. Feed it 5 times (should not reset)
//! 4. Stop feeding -> should reset after 2s (test ends with reset)
//!
//! Pass: Step 3 completes, LED blinks before reset.
//! Fail: Early reset (WDT config wrong) or no reset (WDT broken).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const LP_WDT_BASE: u32 = 0x5011_6000; // LP_WDT peripheral (per esp32p4 PAC -- 0x5011_3000 is LP_ANA)
const LP_WDT_CONFIG0: u32 = LP_WDT_BASE + 0x00;
const LP_WDT_CONFIG1: u32 = LP_WDT_BASE + 0x04;
const LP_WDT_FEED: u32 = LP_WDT_BASE + 0x14;
const LP_WDT_WPROTECT: u32 = LP_WDT_BASE + 0x18;
const WKEY: u32 = 0x50D8_3AA1;

fn wdt_unlock() { unsafe { (LP_WDT_WPROTECT as *mut u32).write_volatile(WKEY); } }
fn wdt_lock() { unsafe { (LP_WDT_WPROTECT as *mut u32).write_volatile(0); } }
fn wdt_feed() { unsafe { (LP_WDT_FEED as *mut u32).write_volatile(1); } }

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_wdt: Watchdog timer ===");

    // 1. Verify WDT is disabled (init should have disabled it)
    let config0 = unsafe { (LP_WDT_CONFIG0 as *const u32).read_volatile() };
    info!("LP_WDT CONFIG0: 0x{:08X} (should be 0 = disabled)", config0);
    assert!(config0 & (1 << 31) == 0, "WDT should be disabled after init");
    info!("WDT disabled after init: OK");

    // 2. Enable WDT with ~2 second timeout
    // Stage0 = ResetSystem (action=4), timeout in RTC slow clock ticks
    // RTC slow clock ~136kHz, so 2s ~ 272000 ticks
    info!("Enabling LP_WDT with ~2s timeout...");
    wdt_unlock();
    unsafe {
        // Config1: stage0 hold time (ticks)
        (LP_WDT_CONFIG1 as *mut u32).write_volatile(272_000);
        // Config0: enable + stage0=reset_system(4) + pause_in_slp
        let cfg = (1 << 31)  // wdt_en
            | (4 << 28)      // wdt_stg0 = ResetSystem
            | (7 << 16)      // cpu_reset_length
            | (7 << 13)      // sys_reset_length
            | (1 << 9);      // pause_in_slp
        (LP_WDT_CONFIG0 as *mut u32).write_volatile(cfg);
    }
    wdt_lock();
    info!("LP_WDT enabled");

    // 3. Feed 5 times (every 500ms, well within 2s timeout)
    for i in 0..5 {
        esp32p4_hal_testing::delay_ms(500);
        wdt_unlock();
        wdt_feed();
        wdt_lock();
        info!("WDT feed #{}", i + 1);
    }
    info!("WDT feed test: OK (no reset during feeding)");

    // 4. Signal pass
    esp32p4_hal_testing::signal_pass();
    info!("=== test_wdt: PASS ===");
    info!("Now stopping feed -- board should reset in ~2 seconds...");

    // Don't feed -- WDT should reset the system
    loop {
        esp32p4_hal_testing::delay_ms(100);
    }
}
