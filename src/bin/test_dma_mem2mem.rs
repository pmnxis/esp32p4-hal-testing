// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: AHB DMA mem2mem transfer on all 3 channels.
//!
//! Also serves as errata DMA-767 verification:
//!   v3.0: CH0 has transaction ID overlap with RMT (may cause APM issues)
//!   v3.1: fixed, all channels should work
//!
//! Test: Copy 256 bytes from src to dst via DMA, verify data integrity.
//! Repeats for CH0, CH1, CH2 to compare behavior.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

/// AHB DMA linked-list descriptor (32 bytes, word aligned).
/// Ref: esp-idf gdma_ll.h, TRM v0.5 Ch 6
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct DmaDescriptor {
    /// Word 0: [31:30]=suc_eof/owner, [29:12]=size, [11:0]=length
    config: u32,
    /// Word 1: buffer address
    buf_addr: u32,
    /// Word 2: next descriptor address (0 = end of chain)
    next_desc: u32,
}

const DMA_DESC_OWNER_DMA: u32 = 1 << 31;
const DMA_DESC_SUC_EOF: u32 = 1 << 30;

impl DmaDescriptor {
    const fn new() -> Self {
        Self { config: 0, buf_addr: 0, next_desc: 0 }
    }

    fn setup(&mut self, buf: *const u8, len: usize, eof: bool) {
        let size = (len as u32) << 12; // size field [29:12]
        let length = len as u32 & 0xFFF; // length field [11:0]
        self.config = DMA_DESC_OWNER_DMA | size | length | if eof { DMA_DESC_SUC_EOF } else { 0 };
        self.buf_addr = buf as u32;
        self.next_desc = 0;
    }
}

const XFER_SIZE: usize = 256;

// Static buffers and descriptors (must be in SRAM, not stack)
static mut SRC_BUF: [u8; XFER_SIZE] = [0; XFER_SIZE];
static mut DST_BUF: [u8; XFER_SIZE] = [0; XFER_SIZE];
static mut TX_DESC: DmaDescriptor = DmaDescriptor::new();
static mut RX_DESC: DmaDescriptor = DmaDescriptor::new();

fn dma_mem2mem_test(ch: usize) -> bool {
    let ahb_dma = unsafe { &*esp32p4::AHB_DMA::PTR };

    // Fill src with pattern, clear dst
    unsafe {
        let src = &raw mut SRC_BUF;
        let dst = &raw mut DST_BUF;
        for i in 0..XFER_SIZE {
            (*src)[i] = (i as u8).wrapping_add(ch as u8 * 0x40);
            (*dst)[i] = 0;
        }
    }

    // Setup descriptors
    unsafe {
        let tx = &raw mut TX_DESC;
        let rx = &raw mut RX_DESC;
        let src = &raw const SRC_BUF;
        let dst = &raw const DST_BUF;
        (*tx).setup((*src).as_ptr(), XFER_SIZE, true);
        (*rx).setup((*dst).as_ptr(), XFER_SIZE, true);
    }

    let dma_ch = ahb_dma.ch(ch);

    // Reset channel
    dma_ch.out_conf0().modify(|r, w| unsafe { w.bits(r.bits() | (1 << 2)) }); // out_rst
    dma_ch.out_conf0().modify(|r, w| unsafe { w.bits(r.bits() & !(1 << 2)) });
    dma_ch.in_conf0().modify(|r, w| unsafe { w.bits(r.bits() | (1 << 2)) }); // in_rst
    dma_ch.in_conf0().modify(|r, w| unsafe { w.bits(r.bits() & !(1 << 2)) });

    // Set mem2mem mode: OUT_CONF0.out_mem_trans_en = 1
    dma_ch.out_conf0().modify(|r, w| unsafe { w.bits(r.bits() | (1 << 4)) }); // mem_trans_en

    // Set descriptor addresses
    unsafe {
        let tx_addr = &raw const TX_DESC as u32;
        let rx_addr = &raw const RX_DESC as u32;
        ahb_dma.out_link_addr_ch(ch).write(|w| w.bits(tx_addr));
        ahb_dma.in_link_addr_ch(ch).write(|w| w.bits(rx_addr));
    }

    // Set peri_sel to mem2mem (0x3F = no peripheral, mem2mem)
    dma_ch.out_peri_sel().write(|w| unsafe { w.bits(0x3F) });
    dma_ch.in_peri_sel().write(|w| unsafe { w.bits(0x3F) });

    // Start: OUT_LINK.start = 1, IN_LINK.start = 1
    dma_ch.in_link().modify(|r, w| unsafe { w.bits(r.bits() | (1 << 27)) }); // inlink_start
    dma_ch.out_link().modify(|r, w| unsafe { w.bits(r.bits() | (1 << 27)) }); // outlink_start

    // Wait for transfer complete (poll IN suc_eof interrupt raw)
    let int_ch = ahb_dma.in_int_ch(ch);
    let mut timeout = 1_000_000u32;
    while int_ch.raw().read().in_suc_eof().bit_is_clear() {
        timeout -= 1;
        if timeout == 0 {
            info!("  CH{}: DMA timeout!", ch);
            return false;
        }
    }
    // Clear interrupt
    int_ch.clr().write(|w| w.in_suc_eof().clear_bit_by_one());

    // Verify data
    let mut errors = 0u32;
    unsafe {
        let src = &raw const SRC_BUF;
        let dst = &raw const DST_BUF;
        for i in 0..XFER_SIZE {
            if (*dst)[i] != (*src)[i] {
                if errors < 5 {
                    info!("  CH{}: mismatch at [{}]: src=0x{:02X} dst=0x{:02X}",
                        ch, i, (*src)[i], (*dst)[i]);
                }
                errors += 1;
            }
        }
    }

    if errors == 0 {
        info!("  CH{}: {} bytes transferred OK", ch, XFER_SIZE);
        true
    } else {
        info!("  CH{}: {} errors in {} bytes", ch, errors, XFER_SIZE);
        false
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    let rev = esp_hal::efuse::chip_revision();
    info!("=== test_dma_mem2mem: AHB DMA mem2mem on CH0/1/2 ===");
    info!("Chip revision: v{}.{}", rev.major, rev.minor);

    // Enable AHB DMA clock
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.soc_clk_ctrl1().modify(|_, w| w.ahb_pdma_sys_clk_en().set_bit());
    esp32p4_hal_testing::busy_delay(1000);

    if rev.major == 3 && rev.minor == 0 {
        info!("v3.0: DMA-767 active -- CH0 may have issues with APM");
    }

    let mut all_pass = true;
    for ch in 0..3 {
        info!("--- DMA CH{} mem2mem test ---", ch);
        if !dma_mem2mem_test(ch) {
            all_pass = false;
            if ch == 0 && rev.major == 3 && rev.minor == 0 {
                info!("CH0 failure on v3.0 may be DMA-767 (transaction ID overlap)");
            }
        }
    }

    if all_pass {
        info!("All 3 channels: PASS");
        if rev.major >= 3 && rev.minor >= 1 {
            info!("DMA-767 fix CONFIRMED (CH0 works on v3.1).");
        }
        esp32p4_hal_testing::signal_pass();
    } else {
        info!("Some channels FAILED");
        esp32p4_hal_testing::signal_fail();
    }

    info!("=== test_dma_mem2mem: DONE ===");

    esp32p4_hal_testing::park_alive("test_dma_mem2mem");
}
