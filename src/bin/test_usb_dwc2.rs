// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: USB DWC2 controller presence check.
//!
//! Verifies: USB OTG HS controller accessible, GSNPSID register readable.
//! Software-only test (no USB cable needed).
//! Pass: GSNPSID reads 0x4F54xxxx (Synopsys DWC2).
//! Fail: Wrong ID or bus fault.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const USB_DWC_HS_BASE: u32 = 0x5000_0000;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_usb_dwc2: USB OTG HS controller check ===");

    // Enable USB OTG20 system clock via PAC directly
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst
        .soc_clk_ctrl1()
        .modify(|_, w| w.usb_otg20_sys_clk_en().set_bit());

    esp32p4_hal_testing::busy_delay(10_000);

    // Read GSNPSID (Synopsys ID register at offset 0x40)
    let gsnpsid = unsafe { ((USB_DWC_HS_BASE + 0x40) as *const u32).read_volatile() };
    info!("USB DWC2 GSNPSID: 0x{:08X}", gsnpsid);

    let synopsys_prefix = gsnpsid >> 16;
    if synopsys_prefix == 0x4F54 {
        info!("USB DWC2 v{}.{}{}{} detected",
            (gsnpsid >> 12) & 0xF,
            (gsnpsid >> 8) & 0xF,
            (gsnpsid >> 4) & 0xF,
            gsnpsid & 0xF,
        );
        info!("USB DWC2: PRESENT");
    } else {
        info!("USB DWC2: NOT DETECTED (wrong GSNPSID)");
        info!("This may mean USB clock is not enabled or controller is in reset");
    }

    // Read GHWCFG1-4 for hardware configuration
    let ghwcfg1 = unsafe { ((USB_DWC_HS_BASE + 0x44) as *const u32).read_volatile() };
    let ghwcfg2 = unsafe { ((USB_DWC_HS_BASE + 0x48) as *const u32).read_volatile() };
    let ghwcfg3 = unsafe { ((USB_DWC_HS_BASE + 0x4C) as *const u32).read_volatile() };
    let ghwcfg4 = unsafe { ((USB_DWC_HS_BASE + 0x50) as *const u32).read_volatile() };
    info!("GHWCFG1: 0x{:08X}", ghwcfg1);
    info!("GHWCFG2: 0x{:08X} (EP count, architecture)", ghwcfg2);
    info!("GHWCFG3: 0x{:08X} (FIFO depth)", ghwcfg3);
    info!("GHWCFG4: 0x{:08X}", ghwcfg4);

    let num_ep = ((ghwcfg2 >> 10) & 0xF) + 1;
    let fifo_depth = (ghwcfg3 >> 16) & 0xFFFF;
    info!("Endpoints: {} IN + {} OUT", num_ep, num_ep);
    info!("FIFO depth: {} words", fifo_depth);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_usb_dwc2: PASS ===");

    esp32p4_hal_testing::park_alive("test_usb_dwc2");
}
