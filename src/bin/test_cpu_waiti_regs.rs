// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: P4 CPU WAITI control registers.
//!
//! Confirms the two registers esp-hal relies on for WFI/debugger safety
//! are actually live at their documented addresses on v3.2/ECO7 silicon.
//!
//!   1. HP_SYS.CPU_WAITI_CONF         @ 0x500E_5118  (in PAC, used by esp-hal)
//!   2. HP_SYS_CLKRST.CPU_WAITI_CTRL0 @ 0x500E_60F4  (hw_ver3, NOT in PAC yet)
//!
//! esp-hal's `cpu_wait_mode_on()` reads (1) as a master `force_on` bit that
//! overrides the per-core gating configured in (2). This test reports both
//! values and checks the reset defaults.
//!
//! Pass: HP_SYS force_on=1, HP_SYS_CLKRST raw value = 0x3.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const HP_SYS_CPU_WAITI_CONF_ADDR: *const u32 = 0x500E_5118 as *const u32;
const HP_SYS_CLKRST_CPU_WAITI_CTRL0_ADDR: *const u32 = 0x500E_60F4 as *const u32;

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_cpu_waiti_regs: P4 WAITI registers ===");

    // HP_SYS.CPU_WAITI_CONF -- master "force_on" bit that esp-hal reads
    let hp_sys = unsafe { core::ptr::read_volatile(HP_SYS_CPU_WAITI_CONF_ADDR) };
    let force_on = (hp_sys & 0x1) != 0;
    let delay_num = (hp_sys >> 1) & 0xF;
    info!(
        "HP_SYS.CPU_WAITI_CONF         = 0x{:08x}  force_on={}  delay_num={}",
        hp_sys, force_on, delay_num
    );

    // HP_SYS_CLKRST.CPU_WAITI_CTRL0 -- hw_ver3 per-core fine-grained bits
    let clkrst = unsafe { core::ptr::read_volatile(HP_SYS_CLKRST_CPU_WAITI_CTRL0_ADDR) };
    let core0_icg_en = (clkrst & 0x1) != 0;
    let core1_icg_en = (clkrst & 0x2) != 0;
    info!(
        "HP_SYS_CLKRST.CPU_WAITI_CTRL0 = 0x{:08x}  CORE0_ICG_EN={} CORE1_ICG_EN={}",
        clkrst, core0_icg_en, core1_icg_en
    );

    // Reset defaults, per IDF hw_ver3 headers:
    //   hp_system_reg.h:943    HP_CPU_WAIT_MODE_FORCE_ON   default 1
    //   hp_sys_clkrst_reg.h:4380,4390  both CORE{0,1}_WAITI_ICG_EN default 1
    assert!(force_on, "HP_SYS.CPU_WAITI_CONF.force_on should be 1 at reset");
    assert_eq!(
        clkrst & 0x3,
        0x3,
        "HP_SYS_CLKRST.CPU_WAITI_CTRL0 both ICG_EN bits should be 1 at reset"
    );

    info!("");
    info!("Implication for esp-hal cpu_wait_mode_on():");
    info!("  HP_SYS.force_on=1 -> cpu_waiti_clk forced on -> WFI won't gate");
    info!("  -> safe to WFI even with debugger attached");
    info!("  (master bit overrides CLKRST per-core ICG_EN configuration)");

    esp32p4_hal_testing::signal_pass();
    info!("=== test_cpu_waiti_regs: PASS ===");

    esp32p4_hal_testing::park_alive("test_cpu_waiti_regs");
}
