// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errata test: DMA-767 -- DMA CH0 transaction ID overlap with RMT.
//!
//! Bug: AHB DMA channel 0 mem2mem transfer uses transaction ID 'a',
//!      same as RMT peripheral. APM can't distinguish them.
//! Affected: v3.0 only. Fixed in v3.1.
//!
//! Test strategy:
//!   1. Configure AHB DMA CH0 for mem2mem transfer
//!   2. Copy a known buffer SRAM -> SRAM via DMA CH0
//!   3. Verify data integrity
//!   4. Repeat with CH1 and CH2
//!   5. On v3.0: CH0 may have permission issues if APM is active
//!   6. On v3.1: all channels should work identically
//!
//! Also useful as a basic DMA functionality test regardless of errata.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

/// AHB DMA base (GDMA v2 compatible)
const AHB_DMA_BASE: u32 = 0x5008_5000;

/// DMA channel register offsets (from AHB DMA base)
/// Each channel: CH(n) cluster at base + 0x40 + n * channel_stride
/// Ref: esp-idf ahb_dma_ll.h, PAC ahb_dma/ch/

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let rev = esp_hal::efuse::chip_revision();
    info!("=== test_errata_dma767: DMA CH0 transaction ID overlap ===");
    info!("Chip revision: v{}.{}", rev.major, rev.minor);

    if rev.major == 3 && rev.minor == 0 {
        info!("v3.0: DMA-767 IS ACTIVE. CH0 mem2mem has same transaction ID as RMT.");
        info!("Workaround: use CH1 or CH2 for mem2mem, not CH0.");
    } else {
        info!("v3.1+: DMA-767 should be FIXED.");
    }

    // Enable AHB DMA clock
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.soc_clk_ctrl1().modify(|_, w| w.ahb_pdma_sys_clk_en().set_bit());
    esp32p4_hal_testing::busy_delay(1000);

    // Verify AHB DMA is accessible
    let ahb_dma = unsafe { &*esp32p4::AHB_DMA::PTR };
    let misc = ahb_dma.misc_conf().read().bits();
    info!("AHB_DMA MISC_CONF: 0x{:08X}", misc);
    info!("AHB_DMA accessible: OK");

    // TODO(P4X): Full DMA mem2mem test
    // 1. Allocate src buffer (SRAM, 256 bytes, known pattern)
    // 2. Allocate dst buffer (SRAM, 256 bytes, zeros)
    // 3. Configure DMA CH0 descriptors:
    //    - TX descriptor: src addr, length, owner=DMA
    //    - RX descriptor: dst addr, length, owner=DMA
    // 4. Set CH0 IN/OUT peri_sel to mem2mem
    // 5. Start DMA transfer
    // 6. Wait for completion
    // 7. Compare src vs dst
    // 8. Repeat for CH1, CH2
    //
    // DMA descriptor format matches our descriptors.rs in emac module.
    // AHB DMA uses same descriptor format as GDMA.

    info!("DMA CH0 register access test...");
    // Read CH0 out_conf0
    let ch0 = ahb_dma.ch(0);
    let out_conf0 = ch0.out_conf0().read().bits();
    info!("CH0 OUT_CONF0: 0x{:08X}", out_conf0);

    let ch1 = ahb_dma.ch(1);
    let out_conf1 = ch1.out_conf0().read().bits();
    info!("CH1 OUT_CONF0: 0x{:08X}", out_conf1);

    let ch2 = ahb_dma.ch(2);
    let out_conf2 = ch2.out_conf0().read().bits();
    info!("CH2 OUT_CONF0: 0x{:08X}", out_conf2);

    info!("All 3 DMA channels accessible: OK");
    info!("DMA mem2mem transfer: TODO (need descriptor setup)");

    if rev.major == 3 && rev.minor == 0 {
        info!("RECOMMENDATION: Use CH1 or CH2 for mem2mem on v3.0");
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_errata_dma767: DONE ===");

    esp32p4_hal_testing::park_alive("test_errata_dma767");
}
