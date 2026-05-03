// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal entry: clear LP_SYS.usb_ctrl USB-PHY MUX bits, then spin.
//!
//! Hypothesis: something earlier in this session set
//! `LP_SYS.usb_ctrl.sw_hw_usb_phy_sel` and/or `sw_usb_phy_sel`,
//! routing USJ to USB-PHY-1 (away from the EV board's USB-C connector).
//! LP_SYS lives in the LP_AON power domain and does NOT reset on a plain
//! HP RST -- only on POR. So once these bits are set, every RST keeps
//! USJ disconnected from the connector.
//!
//! Fix: write 0 to LP_SYS.usb_ctrl (0x5011_0100) at boot to restore the
//! HW-controlled USJ-on-PHY0 default. After this binary boots once, USJ
//! should re-enumerate on subsequent RST.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();

const LP_SYS_USB_CTRL: u32 = 0x5011_0100;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    // Clear sw_hw_usb_phy_sel (bit 0) and sw_usb_phy_sel (bit 1) so the
    // USB-PHY MUX falls back to its hw-controlled default (USJ on PHY0).
    unsafe {
        (LP_SYS_USB_CTRL as *mut u32).write_volatile(0);
    }

    loop {
        core::hint::spin_loop();
    }
}
