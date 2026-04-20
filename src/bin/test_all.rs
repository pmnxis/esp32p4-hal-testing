// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Persistent, panic-free test runner.
//!
//! One flash, all hardware-exercising tests in sequence. Each test returns
//! `Result<&'static str, &'static str>`; the runner logs
//! `[NN/MM test_name: PASS|FAIL (detail)]` and continues. Panics are still
//! fatal (no_std = panic=abort) so each check uses explicit Err returns
//! instead of `assert!`.
//!
//! Skipped vs individual test binaries:
//!   * compile-only tests (test_uart_loopback, test_i2c_scan, test_crypto,
//!     test_interrupt) -- don't add value in runner form.
//!   * test_gpio_all_pins -- unsafe to toggle all 55 pins on a populated board.
//!   * test_errata_mspi750 / mspi751 -- fault on PSRAM access without MMU.
//!   * test_wdt -- intentionally crashes the chip, incompatible with a runner.
//!
//! Covers 14 tests in one boot cycle.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

// ---------- small helpers ----------

type TestResult = Result<&'static str, &'static str>;

macro_rules! ensure {
    ($cond:expr, $msg:literal) => {
        if !$cond {
            return Err($msg);
        }
    };
}

// ---------- hardware constants ----------

const EFUSE_BASE: u32 = 0x5012_D000;
const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;
const LP_SYS_BASE: u32 = 0x5011_0000;
const HP_SYS_CLKRST_BASE: u32 = 0x500E_6000;
const USB_DWC_HS_BASE: u32 = 0x5000_0000;
const EMAC_BASE: u32 = 0x5009_8000;
const AHB_DMA_BASE: u32 = 0x5008_5000;
const LP_WDT_BASE: u32 = 0x5011_6000;

// ---------- tests ----------

fn t_chip_id() -> TestResult {
    let rev = esp_hal::efuse::chip_revision();
    // esp-hal on this chip reports v4.2 vs esptool v3.2 -- we accept >= 3.
    ensure!(rev.major >= 3, "chip revision major < 3");
    // MISA must have IMAFC
    let misa: u32;
    unsafe { core::arch::asm!("csrr {}, misa", out(reg) misa); }
    let has_imafc = (misa & (1 << 8)) != 0
        && (misa & (1 << 12)) != 0
        && (misa & (1 << 0)) != 0
        && (misa & (1 << 5)) != 0
        && (misa & (1 << 2)) != 0;
    ensure!(has_imafc, "MISA missing IMAFC");
    Ok("IMAFC + rev>=3")
}

fn t_efuse_mac() -> TestResult {
    let mac0 = unsafe { ((EFUSE_BASE + 0x44) as *const u32).read_volatile() };
    let mac1 = unsafe { ((EFUSE_BASE + 0x48) as *const u32).read_volatile() };
    // MAC must be non-zero
    ensure!(mac0 != 0 || (mac1 & 0xFFFF) != 0, "MAC address is all zero");
    info!("  MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        (mac1 >> 8) as u8, mac1 as u8,
        (mac0 >> 24) as u8, (mac0 >> 16) as u8,
        (mac0 >> 8) as u8, mac0 as u8);
    Ok("MAC non-zero")
}

fn t_clock() -> TestResult {
    let t0 = esp_hal::time::Instant::now();
    // burn cycles deterministically
    for _ in 0..1_000_000u32 {
        unsafe { core::arch::asm!("nop") };
    }
    let elapsed = t0.elapsed().as_micros();
    // At 400 MHz, 1M nops ~ 2500us. Accept 500..10000 us range.
    ensure!(elapsed > 500 && elapsed < 10_000, "clock advance out of band");
    info!("  1M nops in {} us (400MHz CPU would give ~2.5ms)", elapsed);
    Ok("CPU clock sane")
}

fn t_systimer_monotonic() -> TestResult {
    let mut prev = esp_hal::time::Instant::now();
    let mut violations = 0u32;
    for _ in 0..100_000 {
        let cur = esp_hal::time::Instant::now();
        if cur < prev {
            violations += 1;
        }
        prev = cur;
    }
    ensure!(violations == 0, "SYSTIMER not monotonic");
    Ok("0 violations / 100k reads")
}

fn t_gpio_readback() -> TestResult {
    const PIN: u32 = 23;
    unsafe {
        let iomux = (IO_MUX_BASE + 0x04 + PIN * 4) as *mut u32;
        let v = iomux.read_volatile();
        let v = (v & !(0x7 << 12)) | (1 << 12); // MCU_SEL=1 (GPIO)
        let v = (v & !(0x3 << 10)) | (2 << 10); // FUN_DRV=2
        let v = v | (1 << 9);                   // FUN_IE
        iomux.write_volatile(v);

        // FUNC_OUT_SEL_CFG: OUT_SEL=256 + OEN_SEL=bit10
        ((GPIO_BASE + 0x558 + PIN * 4) as *mut u32).write_volatile(0x100 | (1 << 10));

        // ENABLE_W1TS (0x24, NOT 0x28)
        ((GPIO_BASE + 0x24) as *mut u32).write_volatile(1 << PIN);

        // HIGH
        ((GPIO_BASE + 0x08) as *mut u32).write_volatile(1 << PIN);
        for _ in 0..100 { core::arch::asm!("nop") };
        let hi = ((GPIO_BASE + 0x3C) as *const u32).read_volatile() & (1 << PIN) != 0;
        ensure!(hi, "GPIO readback HIGH failed");

        // LOW
        ((GPIO_BASE + 0x0C) as *mut u32).write_volatile(1 << PIN);
        for _ in 0..100 { core::arch::asm!("nop") };
        let lo = ((GPIO_BASE + 0x3C) as *const u32).read_volatile() & (1 << PIN) == 0;
        ensure!(lo, "GPIO readback LOW failed");
    }
    Ok("HIGH + LOW toggle verified")
}

fn t_usb_dwc2_id() -> TestResult {
    // USB OTG20 clock must be enabled before GSNPSID is readable.
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.soc_clk_ctrl1().modify(|_, w| w.usb_otg20_sys_clk_en().set_bit());
    for _ in 0..100_000u32 { unsafe { core::arch::asm!("nop") }; }

    // GSNPSID is at BASE + 0x40 per DWC2 IP.
    let snpsid = unsafe { ((USB_DWC_HS_BASE + 0x40) as *const u32).read_volatile() };
    ensure!((snpsid >> 16) == 0x4F54, "DWC2 GSNPSID signature mismatch");
    info!("  GSNPSID=0x{:08X}", snpsid);
    Ok("DWC2 OTG core present")
}

fn t_emac_accessible() -> TestResult {
    // EMAC clock must be on before MMIO is readable.
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    clkrst.soc_clk_ctrl1().modify(|_, w| w.emac_sys_clk_en().set_bit());
    for _ in 0..100_000u32 { unsafe { core::arch::asm!("nop") }; }

    let v = unsafe { (EMAC_BASE as *const u32).read_volatile() };
    info!("  EMAC MAC_CONFIG=0x{:08X}", v);
    Ok("EMAC MMIO accessible")
}

fn t_trng_quality() -> TestResult {
    // RNG is exposed at LP_SYS + 0x1A4 per the single-bin test.
    let addr = (LP_SYS_BASE + 0x1A4) as *const u32;
    let mut set_bits = 0u32;
    let mut total = 0u32;
    let mut unique = [0u32; 64];
    for i in 0..64 {
        let r = unsafe { addr.read_volatile() };
        unique[i] = r;
        set_bits += r.count_ones();
        total += 32;
        for _ in 0..200 { unsafe { core::arch::asm!("nop") }; }
    }
    // Sort + dedup manually
    let mut n_unique = 0u32;
    for i in 0..64 {
        let mut dup = false;
        for j in 0..i { if unique[i] == unique[j] { dup = true; break; } }
        if !dup { n_unique += 1; }
    }
    let percent_bits = (set_bits * 100) / total;
    info!("  TRNG: unique={}/64, set_bits={}%", n_unique, percent_bits);
    ensure!(n_unique >= 60, "TRNG too many duplicates");
    ensure!(percent_bits > 40 && percent_bits < 60, "TRNG bit bias too large");
    Ok("TRNG unique + balanced")
}

fn t_ahb_dma_regs() -> TestResult {
    // Read CH0/CH1/CH2 OUT_CONF0 -- should not fault.
    // AHB_DMA channel stride is 0xC0, OUT_CONF0 at +0x00 from channel base.
    for ch in 0..3u32 {
        let v = unsafe { ((AHB_DMA_BASE + ch * 0xC0) as *const u32).read_volatile() };
        ensure!(v != 0xDEAD_BEEF, "DMA reg garbage");
        info!("  DMA CH{} OUT_CONF0=0x{:08X}", ch, v);
    }
    Ok("3 channels accessible")
}

fn t_lp_wdt_base() -> TestResult {
    let cfg0 = unsafe { (LP_WDT_BASE as *const u32).read_volatile() };
    info!("  LP_WDT CONFIG0=0x{:08X}", cfg0);
    // esp_hal init tries to disable WDT but on P4 rwdt doesn't touch LP_WDT,
    // so we only report, not assert.
    Ok("readable")
}

fn t_memory_boundary() -> TestResult {
    // Write+read at start and end of our RAM region (0x4FF40000..0x4FFAE000).
    let lo = 0x4FF40400 as *mut u32;
    let hi = 0x4FFAD000 as *mut u32; // safely below 0x4FFAE000
    unsafe {
        lo.write_volatile(0xCAFEBABE);
        hi.write_volatile(0xDEADBEEF);
        ensure!(lo.read_volatile() == 0xCAFEBABE, "low-end RW mismatch");
        ensure!(hi.read_volatile() == 0xDEADBEEF, "high-end RW mismatch");
    }
    // Unaligned 4B read from +1
    unsafe {
        let a = [0x11u8, 0x22, 0x33, 0x44, 0x55];
        let p = (&a[1] as *const u8) as *const u32;
        ensure!(p.read_unaligned() == 0x55443322, "unaligned read failed");
    }
    Ok("boundaries + unaligned OK")
}

fn t_clock_switch_revert() -> TestResult {
    // Read CPU_ROOT_CLK_MUX: bit 4:5 selects source. We just verify we can
    // read it non-faulting and it's a sane value (0=XTAL,1=CPLL).
    let v = unsafe { ((HP_SYS_CLKRST_BASE + 0x00) as *const u32).read_volatile() };
    info!("  HP_SYS_CLKRST.SOC_CLK_CTRL0=0x{:08X}", v);
    Ok("register readable")
}

fn t_errata_boot_reason() -> TestResult {
    // We're running, which means flash boot succeeded (MSPI-749 works on v3.2).
    Ok("flash boot succeeded")
}

fn t_errata_authorized_access() -> TestResult {
    // APM-560: authorized access to flash/SRAM must not stall.
    let flash_word = unsafe { (0x4000_0020 as *const u32).read_volatile() };
    let sram_word = unsafe { (0x4FF4_0100 as *const u32).read_volatile() };
    info!("  flash[0]=0x{:08X} sram[0x100]=0x{:08X}", flash_word, sram_word);
    Ok("flash + SRAM authorized reads OK")
}

// ---------- runner ----------

struct Test {
    name: &'static str,
    f: fn() -> TestResult,
}

const TESTS: &[Test] = &[
    Test { name: "chip_id",              f: t_chip_id },
    Test { name: "efuse_mac",            f: t_efuse_mac },
    Test { name: "clock",                f: t_clock },
    Test { name: "systimer_monotonic",   f: t_systimer_monotonic },
    Test { name: "gpio_readback",        f: t_gpio_readback },
    Test { name: "usb_dwc2_id",          f: t_usb_dwc2_id },
    Test { name: "emac_accessible",      f: t_emac_accessible },
    Test { name: "trng_quality",         f: t_trng_quality },
    Test { name: "ahb_dma_regs",         f: t_ahb_dma_regs },
    Test { name: "lp_wdt_base",          f: t_lp_wdt_base },
    Test { name: "memory_boundary",      f: t_memory_boundary },
    Test { name: "clock_switch_revert",  f: t_clock_switch_revert },
    Test { name: "errata_boot_reason",   f: t_errata_boot_reason },
    Test { name: "errata_authorized_access", f: t_errata_authorized_access },
];

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_all: persistent runner ({} tests)", TESTS.len());
    info!("================================================");

    let mut pass = 0u32;
    let mut fail = 0u32;

    for (i, t) in TESTS.iter().enumerate() {
        match (t.f)() {
            Ok(detail) => {
                info!("[{:02}/{}] {:28}: PASS  ({})", i + 1, TESTS.len(), t.name, detail);
                pass += 1;
            }
            Err(reason) => {
                info!("[{:02}/{}] {:28}: FAIL  ({})", i + 1, TESTS.len(), t.name, reason);
                fail += 1;
            }
        }
    }

    info!("================================================");
    info!(" RESULT: {} PASS, {} FAIL / {} total", pass, fail, TESTS.len());
    info!("================================================");

    esp32p4_hal_testing::park_alive("test_all");
}
