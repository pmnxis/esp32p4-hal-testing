// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errata test: MSPI-749 -- Load access fault during power-on.
//!
//! Bug: First and second MSPI AXI bus access after power-on may get
//!      error response, causing boot failure.
//! Affected: v3.0. Fixed in v3.1.
//!
//! Test strategy:
//!   If we're running, the boot succeeded.
//!   On v3.0: boot may have required WDT-triggered second attempt.
//!   On v3.1: boot should succeed on first attempt.
//!
//!   We verify by checking reset reason:
//!   - PowerOn (0x01): clean first boot = v3.1 fix confirmed
//!   - RtcWdt / SuperWdt: WDT reset = may have been MSPI-749 recovery

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let rev = esp_hal::efuse::chip_revision();
    info!("=== test_errata_mspi749: MSPI boot fault ===");
    info!("Chip revision: v{}.{}", rev.major, rev.minor);
    info!("We're running = boot succeeded.");

    // Check reset reason via ROM function (rtc_cntl is private without unstable)
    // Ref: esp-idf rom/rtc.h -- rtc_get_reset_reason(cpu_num)
    //      Returns: 1=PowerOn, 3=SW, 5=DeepSleep, 9=RtcWdt, 0x10=SysRtcWdt, 0x12=SuperWdt
    unsafe extern "C" {
        fn rtc_get_reset_reason(cpu_num: u32) -> u32;
    }
    let reason = unsafe { rtc_get_reset_reason(0) };
    info!("Reset reason code: 0x{:02X}", reason);

    let reason_name = match reason {
        0x01 => "PowerOn",
        0x03 => "SW reset",
        0x05 => "DeepSleep",
        0x09 => "RTC WDT",
        0x0F => "BrownOut",
        0x10 => "SYS RTC WDT",
        0x12 => "SuperWDT",
        _ => "Other",
    };
    info!("Reset reason: {} (0x{:02X})", reason_name, reason);

    if reason == 0x01 {
        info!("Clean power-on boot.");
        if rev.major == 3 && rev.minor == 0 {
            info!("v3.0: Clean boot despite MSPI-749 -- lucky or bug not triggered.");
        } else {
            info!("v3.1+: MSPI-749 fix confirmed (clean boot).");
        }
    } else if reason == 0x09 || reason == 0x10 || reason == 0x12 {
        info!("WDT reset detected.");
        if rev.major == 3 && rev.minor == 0 {
            info!("v3.0: May be MSPI-749 recovery (first boot failed, WDT reset).");
        } else {
            info!("v3.1+: WDT on v3.1 is unexpected for this errata.");
        }
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_errata_mspi749: DONE ===");

    esp32p4_hal_testing::park_alive("test_errata_mspi749");
}
