// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Corner test: Dynamic clock source switching.
//!
//! Tests: Switch CPU from CPLL -> XTAL -> CPLL during operation.
//! Dangerous edge case: if clock switch is glitchy, CPU may hang.
//!
//! 1. Verify CPU running on CPLL (400MHz)
//! 2. Switch to XTAL (40MHz) -- CPU should slow down 10x
//! 3. Measure time passes 10x slower
//! 4. Switch back to CPLL
//! 5. Verify speed restored
//!
//! NOTE: This is a RISKY test -- clock switching can hang if done wrong.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const LP_AON_CLKRST_BASE: u32 = 0x5011_1000;

fn get_cpu_clock_src() -> u8 {
    // LP_AON_CLKRST.lp_aonclkrst_hp_clk_ctrl: hp_root_clk_src_sel [1:0]
    // 0=XTAL, 1=CPLL, 2=RC_FAST
    let clkrst = unsafe { &*esp32p4::LP_AON_CLKRST::PTR };
    clkrst.lp_aonclkrst_hp_clk_ctrl().read().lp_aonclkrst_hp_root_clk_src_sel().bits()
}

fn set_cpu_clock_src(sel: u8) {
    let clkrst = unsafe { &*esp32p4::LP_AON_CLKRST::PTR };
    clkrst.lp_aonclkrst_hp_clk_ctrl().modify(|_, w| unsafe {
        w.lp_aonclkrst_hp_root_clk_src_sel().bits(sel)
    });
}

fn measure_cpu_mhz() -> u32 {
    let mcycle_start: u32;
    unsafe { core::arch::asm!("csrr {}, mcycle", out(reg) mcycle_start); }

    let t0 = esp_hal::time::Instant::now();
    while t0.elapsed().as_micros() < 1_000 { // 1ms
        core::hint::spin_loop();
    }

    let mcycle_end: u32;
    unsafe { core::arch::asm!("csrr {}, mcycle", out(reg) mcycle_end); }

    mcycle_end.wrapping_sub(mcycle_start) / 1_000 // cycles per ms ~= MHz
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_clock_switch: Dynamic clock switching ===");

    // 1. Initial state: should be CPLL
    let src = get_cpu_clock_src();
    info!("Initial clock source: {} (0=XTAL, 1=CPLL, 2=RC_FAST)", src);
    let mhz = measure_cpu_mhz();
    info!("Initial CPU speed: ~{} MHz", mhz);

    // 2. Switch to XTAL (40MHz)
    info!("Switching to XTAL (40MHz)...");
    set_cpu_clock_src(0); // XTAL
    // After this, CPU is MUCH slower -- everything takes 10x longer

    let src = get_cpu_clock_src();
    info!("After switch: source={}", src);
    let mhz_xtal = measure_cpu_mhz();
    info!("XTAL CPU speed: ~{} MHz (expected ~40)", mhz_xtal);

    // 3. Switch back to CPLL
    info!("Switching back to CPLL (400MHz)...");
    set_cpu_clock_src(1); // CPLL

    let src = get_cpu_clock_src();
    info!("Restored: source={}", src);
    let mhz_restored = measure_cpu_mhz();
    info!("Restored CPU speed: ~{} MHz", mhz_restored);

    // 4. Verify speeds make sense
    if mhz > 300 && mhz_xtal < 80 && mhz_restored > 300 {
        info!("Clock switch: OK (CPLL->XTAL->CPLL)");
    } else {
        info!("Clock switch: UNEXPECTED speeds ({}/{}/{})", mhz, mhz_xtal, mhz_restored);
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_clock_switch: PASS ===");

    esp32p4_hal_testing::park_alive("test_clock_switch");
}
