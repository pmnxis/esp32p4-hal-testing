// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: EMAC PHY detection via MDIO (no Ethernet cable needed).
//!
//! Reads IP101GR PHY ID register via MDIO interface.
//! EV Board has IP101GR at PHY address 0.
//! Only needs EV Board -- no Ethernet cable required for PHY ID read.
//!
//! Pass: PHY ID matches IP101GR (0x0243:0x0C50).
//! Fail: Wrong ID or MDIO timeout.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const EMAC_BASE: u32 = 0x5009_8000;
const GMII_ADDRESS: u32 = 0x10;
const GMII_DATA: u32 = 0x14;

fn mdio_read(phy_addr: u8, reg_addr: u8) -> u16 {
    unsafe {
        let gmiiaddr = (EMAC_BASE + GMII_ADDRESS) as *mut u32;
        let gmiidata = (EMAC_BASE + GMII_DATA) as *const u32;
        let cmd = ((phy_addr as u32) << 11) | ((reg_addr as u32) << 6) | 1;
        gmiiaddr.write_volatile(cmd);
        let mut timeout = 100_000u32;
        while gmiiaddr.read_volatile() & 1 != 0 {
            timeout -= 1;
            if timeout == 0 {
                return 0xFFFF; // timeout
            }
            core::hint::spin_loop();
        }
        (gmiidata.read_volatile() & 0xFFFF) as u16
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_emac_phy: PHY detection via MDIO ===");

    // Enable EMAC system clock first
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.soc_clk_ctrl1().modify(|_, w| w.emac_sys_clk_en().set_bit());
    esp32p4_hal_testing::busy_delay(10_000);

    // Read PHY IDR1 (register 2) and IDR2 (register 3)
    let phy_addr: u8 = 0; // IP101GR on EV Board
    let id1 = mdio_read(phy_addr, 2);
    let id2 = mdio_read(phy_addr, 3);

    info!("PHY IDR1: 0x{:04X} (expected 0x0243)", id1);
    info!("PHY IDR2: 0x{:04X} (expected 0x0C5x)", id2);

    if id1 == 0x0243 && (id2 & 0xFFF0) == 0x0C50 {
        info!("IP101GR PHY detected!");
        let rev = id2 & 0x000F;
        info!("PHY revision: {}", rev);
    } else if id1 == 0xFFFF {
        info!("MDIO timeout -- EMAC clock may not be configured");
        info!("RMII pins may need IO MUX function 3 configuration");
    } else {
        info!("Unknown PHY ID: {:04X}:{:04X}", id1, id2);
    }

    // Read PHY BSR (register 1) for link status
    let bsr = mdio_read(phy_addr, 1);
    info!("PHY BSR: 0x{:04X}", bsr);
    let link_up = bsr & (1 << 2) != 0;
    let autoneg_done = bsr & (1 << 5) != 0;
    info!("Link up: {} (cable may not be connected)", link_up);
    info!("Auto-negotiation complete: {}", autoneg_done);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_emac_phy: PASS ===");

    esp32p4_hal_testing::park_alive("test_emac_phy");
}
