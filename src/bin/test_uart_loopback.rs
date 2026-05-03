// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! UART1 loopback test via external GPIO short.
//!
//! Wiring: connect GPIO6 (UART1 TX) to GPIO7 (UART1 RX) with a single
//! jumper wire on the EV board's J1 header.
//!
//! NOTE: GPIO4/GPIO5 cannot be used here because they have a JTAG (MTMS/MTDO)
//! limitation in the P4 metadata; the JTAG hardware path overrides the GPIO
//! matrix output, so UART_TXD never reaches the pin even with `MCU_SEL=1`.
//! GPIO6/7 are clean (only SPI2 alt function on function 3) so the GPIO
//! matrix routing works as expected.
//!
//! Verifies UART1 register-level access on ESP32-P4 v3.x silicon by:
//! 1. Routing UART1_TXD signal -> GPIO4, GPIO5 -> UART1_RXD signal via
//!    GPIO matrix.
//! 2. Configuring UART1 clock + 115200 baud divider.
//! 3. Writing 16 bytes to UART1 TX FIFO.
//! 4. Reading them back from UART1 RX FIFO.
//! 5. Asserting byte-perfect round-trip.
//!
//! Pass: 16/16 bytes match.
//! Fail: any mismatch or short read.
//!
//! NOTE: This is a register-level test (no esp-hal driver). esp-hal's
//! Uart driver requires `unstable` feature, which exposes P4-driver
//! pre-existing build errors orthogonal to this PR. Once those are
//! cleaned up, the test should be rewritten to use `esp_hal::uart::Uart`.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const TX_PIN: u32 = 6;
const RX_PIN: u32 = 7;

// === MMIO base addresses (ESP32-P4 v3.x) ===
const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;
const UART1_BASE: u32 = 0x500C_B000;
const HP_SYS_CLKRST_BASE: u32 = 0x500E_6000;

// GPIO matrix function ID for UART1 (per esp32p4.toml: tx=UART1_TXD, rx=UART1_RXD)
// Lookup from PAC `peripheral_input_signals` / `peripheral_output_signals`:
//   UART1_TXD = output signal id 13
//   UART1_RXD = input  signal id 13
const UART1_TXD_OUT_SIG: u32 = 13;
const UART1_RXD_IN_SIG: u32 = 13;

// UART1 register offsets (a small subset — full layout in esp32p4 PAC `uart::RegisterBlock`)
const UART_FIFO: u32 = 0x00;            // FIFO data (8-bit)
const UART_INT_RAW: u32 = 0x04;
const UART_INT_CLR: u32 = 0x10;
const UART_CLKDIV: u32 = 0x14;
const UART_STATUS: u32 = 0x1C;
const UART_CONF0: u32 = 0x20;
const UART_CONF1: u32 = 0x24;
const UART_CLK_CONF: u32 = 0x88;        // UART_CLK_CONF (sclk source / divider)
const UART_ID: u32 = 0x80;              // UART_ID (also gates register update bit)

// HP_SYS_CLKRST offsets for UART1 clock enable / reset
const HP_SYS_CLKRST_PERI_CLK_CTRL110: u32 = 0x118;  // uart1 clk_en bit
const HP_SYS_CLKRST_HP_RST_EN1: u32 = 0x130;        // uart1 reset bit (active high)

fn mmio_read(addr: u32) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}
fn mmio_write(addr: u32, val: u32) {
    unsafe { (addr as *mut u32).write_volatile(val); }
}
fn mmio_set(addr: u32, mask: u32) {
    let v = mmio_read(addr);
    mmio_write(addr, v | mask);
}
fn mmio_clr(addr: u32, mask: u32) {
    let v = mmio_read(addr);
    mmio_write(addr, v & !mask);
}

/// Set up GPIO matrix: UART1_TXD signal -> TX_PIN output, RX_PIN input -> UART1_RXD signal.
fn setup_gpio_matrix() {
    // TX side: configure TX_PIN as output, drive from UART1_TXD signal via GPIO matrix.
    // IO MUX: select GPIO function (MCU_SEL = 1).
    let iomux_tx = IO_MUX_BASE + 0x04 + TX_PIN * 4;
    let mut v = mmio_read(iomux_tx);
    v = (v & !(0x7 << 12)) | (1 << 12);   // MCU_SEL = 1 (GPIO)
    v = (v & !(0x3 << 10)) | (2 << 10);   // FUN_DRV = 2 (default drive)
    mmio_write(iomux_tx, v);
    // GPIO_FUNC{n}_OUT_SEL_CFG: drive from peripheral signal UART1_TXD_OUT_SIG.
    // OEN_SEL=0 lets the peripheral control output-enable (UART_TXD always-on
    // while UART is enabled) -- avoids race with GPIO_ENABLE_REG.
    mmio_write(GPIO_BASE + 0x558 + TX_PIN * 4, UART1_TXD_OUT_SIG);
    // Still set ENABLE bit as a belt-and-suspenders guard.
    mmio_write(GPIO_BASE + 0x24, 1u32 << TX_PIN);

    // RX side: configure RX_PIN as input, route to UART1_RXD signal via GPIO matrix.
    let iomux_rx = IO_MUX_BASE + 0x04 + RX_PIN * 4;
    let mut v = mmio_read(iomux_rx);
    v = (v & !(0x7 << 12)) | (1 << 12);   // MCU_SEL = 1 (GPIO)
    v |= 1 << 9;                           // FUN_IE = 1 (input enable)
    mmio_write(iomux_rx, v);
    // GPIO_FUNC{n}_IN_SEL_CFG: select GPIO matrix input pin for UART1_RXD signal.
    // base + 0x154 + sig_id*4, format: pin_num | (1 << 7) (sel = 1 = use GPIO matrix)
    mmio_write(GPIO_BASE + 0x154 + UART1_RXD_IN_SIG * 4, RX_PIN | (1 << 7));
}

/// UART1 register block view at UART1_BASE (uses uart0::RegisterBlock layout).
fn uart1() -> &'static esp32p4::uart0::RegisterBlock {
    unsafe { &*(UART1_BASE as *const esp32p4::uart0::RegisterBlock) }
}

/// Enable UART1 clock tree:
/// - HP_SYS_CLKRST: sys clk, apb clk, peripheral clk + source + divider
/// - hp_rst_en1: clear UART1 core + apb resets
/// - UART1.clk_conf: TX/RX sclk gates
fn enable_uart1_clock() {
    let clkrst = unsafe { &*esp32p4::HP_SYS_CLKRST::PTR };
    // sys clock + apb clock.
    clkrst.soc_clk_ctrl1().modify(|_, w| w.uart1_sys_clk_en().set_bit());
    clkrst.soc_clk_ctrl2().modify(|_, w| w.uart1_apb_clk_en().set_bit());
    // peri_clk_ctrl111: enable peripheral clock + select XTAL (src=0).
    clkrst.peri_clk_ctrl111().modify(|_, w| unsafe {
        w.uart1_clk_en().set_bit().uart1_clk_src_sel().bits(0)
    });
    // peri_clk_ctrl112: divisor = 1 (no additional division before UART_CLKDIV).
    clkrst.peri_clk_ctrl112().modify(|_, w| unsafe {
        w.uart1_sclk_div_num().bits(0)         // div_num = 0 -> divider = 1
         .uart1_sclk_div_numerator().bits(0)
         .uart1_sclk_div_denominator().bits(0)
    });
    // Clear core+apb reset for UART1.
    clkrst.hp_rst_en1().modify(|_, w| {
        w.rst_en_uart1_core().clear_bit().rst_en_uart1_apb().clear_bit()
    });
    // Enable UART1's internal TX and RX sclk gates AND release TX/RX core resets.
    uart1().clk_conf().modify(|_, w| {
        w.tx_sclk_en().set_bit().rx_sclk_en().set_bit()
         .tx_rst_core().clear_bit().rx_rst_core().clear_bit()
    });
}

/// Configure UART1: 8N1, 115200 baud from XTAL (40 MHz).
fn configure_uart1() {
    // CONF0 layout on P4:
    //   bit[1]      parity_en
    //   bits[3:2]   bit_num     (3 = 8 bits)
    //   bits[5:4]   stop_bit_num (1 = 1 stop bit)
    //   bit[22]     rxfifo_rst
    //   bit[23]     txfifo_rst
    //   (other bits left at reset defaults)
    let conf0 = UART1_BASE + UART_CONF0;
    // Toggle rxfifo_rst + txfifo_rst at bits 22/23.
    let v = mmio_read(conf0);
    mmio_write(conf0, v | (1 << 22) | (1 << 23));
    mmio_write(conf0, v & !((1 << 22) | (1 << 23)));

    // 8N1: bit_num=3 (8 bits), stop_bit_num=1, parity_en=0.
    let mut v = mmio_read(conf0);
    v = (v & !(0x3 << 2)) | (3 << 2);   // bit_num = 3 (8 bits)
    v = (v & !(0x3 << 4)) | (1 << 4);   // stop_bit_num = 1 (1 stop bit)
    v &= !(1 << 1);                      // parity_en = 0 (disable parity)
    mmio_write(conf0, v);

    // Clock source select: XTAL (default in CLK_CONF). Verify divider for 115200.
    // Assume sclk = 40 MHz XTAL. Divider = sclk / (baud * 16) = 40e6 / (115200 * 16) ~= 21.7
    // P4 UART has 12 integer + 4 fractional bits in CLKDIV.
    let baud = 115_200u32;
    let sclk = 40_000_000u32;
    let div_int = sclk / (baud * 16);
    let div_frac = ((sclk % (baud * 16)) * 16) / (baud * 16);
    mmio_write(UART1_BASE + UART_CLKDIV, (div_int & 0xFFFFF) | ((div_frac & 0xF) << 20));

    // Sync register update bit (UART_ID.reg_update).
    mmio_set(UART1_BASE + UART_ID, 1 << 30);
    while (mmio_read(UART1_BASE + UART_ID) & (1 << 30)) != 0 {}
}

fn uart1_write_byte(b: u8) {
    // Wait until txfifo_cnt < some_room. STATUS.txfifo_cnt is bits[25:16] (10 bits, 128 max).
    while (mmio_read(UART1_BASE + UART_STATUS) >> 16) & 0x3FF >= 120 {}
    mmio_write(UART1_BASE + UART_FIFO, b as u32);
}

fn uart1_read_byte_nb() -> Option<u8> {
    let cnt = mmio_read(UART1_BASE + UART_STATUS) & 0x3FF; // rxfifo_cnt bits [9:0]
    if cnt == 0 { return None; }
    Some(mmio_read(UART1_BASE + UART_FIFO) as u8)
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("================================================");
    info!(" test_uart_loopback (register-level)");
    info!(" Wire: GPIO{} (TX)  ----  GPIO{} (RX)", TX_PIN, RX_PIN);
    info!("================================================");

    // Pre-flight: verify the GPIO4 -> GPIO5 jumper wire is actually connected.
    // Configure GPIO4 as plain GPIO output, GPIO5 as plain GPIO input (no matrix
    // routing), then drive GPIO4 HIGH/LOW and read GPIO5.
    info!("step 0: jumper wire pre-check");
    {
        // GPIO4 as output
        let iomux_tx = IO_MUX_BASE + 0x04 + TX_PIN * 4;
        let mut v = mmio_read(iomux_tx);
        v = (v & !(0x7 << 12)) | (1 << 12);
        mmio_write(iomux_tx, v);
        mmio_write(GPIO_BASE + 0x558 + TX_PIN * 4, 0x100 | (1 << 10)); // direct GPIO_OUT
        mmio_write(GPIO_BASE + 0x24, 1u32 << TX_PIN); // ENABLE_W1TS

        // GPIO5 as input
        let iomux_rx = IO_MUX_BASE + 0x04 + RX_PIN * 4;
        let mut v = mmio_read(iomux_rx);
        v = (v & !(0x7 << 12)) | (1 << 12);
        v |= 1 << 9; // FUN_IE
        mmio_write(iomux_rx, v);

        // Drive GPIO4 HIGH, read GPIO5
        mmio_write(GPIO_BASE + 0x08, 1u32 << TX_PIN); // OUT_W1TS
        for _ in 0..100 { unsafe { core::arch::asm!("nop"); } }
        let high_bit = (mmio_read(GPIO_BASE + 0x3C) >> RX_PIN) & 1;

        // Drive GPIO4 LOW, read GPIO5
        mmio_write(GPIO_BASE + 0x0C, 1u32 << TX_PIN); // OUT_W1TC
        for _ in 0..100 { unsafe { core::arch::asm!("nop"); } }
        let low_bit = (mmio_read(GPIO_BASE + 0x3C) >> RX_PIN) & 1;

        info!("  GPIO{} HIGH -> GPIO{} read = {}", TX_PIN, RX_PIN, high_bit);
        info!("  GPIO{} LOW  -> GPIO{} read = {}", TX_PIN, RX_PIN, low_bit);
        if high_bit == 1 && low_bit == 0 {
            info!("  jumper wire OK");
        } else {
            info!("  WARNING: GPIO{} not following GPIO{} - check jumper wire", RX_PIN, TX_PIN);
        }
    }

    info!("step 1: enable_uart1_clock()");
    enable_uart1_clock();
    info!("step 2: setup_gpio_matrix()");
    setup_gpio_matrix();
    info!("step 3: configure_uart1()");
    configure_uart1();
    info!("step 4: read UART1 ID");

    // Verify UART1 is reachable.
    let id = mmio_read(UART1_BASE + UART_ID);
    info!("UART1 ID register: 0x{:08X}", id);

    // Check that UART1 is driving GPIO4 (TX). After configuration with no
    // pending transmission, the TX line should be idle HIGH (UART idle = 1).
    // If we read 0, UART_TXD signal isn't reaching GPIO4 via the GPIO matrix.
    let tx_idle = (mmio_read(GPIO_BASE + 0x3C) >> TX_PIN) & 1;
    info!("GPIO{} idle level (UART TX should be 1): {}", TX_PIN, tx_idle);

    // Verify GPIO matrix OUT_SEL config got written.
    let out_sel = mmio_read(GPIO_BASE + 0x558 + TX_PIN * 4);
    info!("GPIO_FUNC{}_OUT_SEL_CFG: 0x{:08X} (expect signal_id={} | OEN bit10)",
        TX_PIN, out_sel, UART1_TXD_OUT_SIG);

    // Verify GPIO matrix IN_SEL config.
    let in_sel = mmio_read(GPIO_BASE + 0x154 + UART1_RXD_IN_SIG * 4);
    info!("GPIO_FUNC_IN_SEL_CFG[sig={}]: 0x{:08X} (expect pin={} | bit7)",
        UART1_RXD_IN_SIG, in_sel, RX_PIN);

    // Read UART1 STATUS to see FIFO levels and TXD/RXD bits.
    let status = mmio_read(UART1_BASE + UART_STATUS);
    info!("UART1 STATUS: 0x{:08X} (txfifo_cnt={}, rxfifo_cnt={})",
        status, (status >> 16) & 0xFF, status & 0xFF);
    info!("  status.txd (bit31): {}, status.rxd (bit15): {}",
        (status >> 31) & 1, (status >> 15) & 1);

    // Verify GPIO ENABLE register bit for TX_PIN.
    let enable_reg = mmio_read(GPIO_BASE + 0x20); // GPIO_ENABLE_REG (raw, not W1TS)
    info!("GPIO_ENABLE_REG bit{} = {}", TX_PIN, (enable_reg >> TX_PIN) & 1);

    // Verify UART register read/write actually works:
    //   1. Read CLKDIV (we just wrote this).
    //   2. Read CONF0.
    let clkdiv = mmio_read(UART1_BASE + UART_CLKDIV);
    let conf0 = mmio_read(UART1_BASE + UART_CONF0);
    info!("UART1 CLKDIV: 0x{:08X} (expect 0x{:08X})", clkdiv,
        (40_000_000 / (115_200 * 16)) | ((((40_000_000 % (115_200 * 16)) * 16) / (115_200 * 16)) << 20));
    info!("UART1 CONF0: 0x{:08X}", conf0);

    // Write a byte and IMMEDIATELY read STATUS to confirm FIFO accepts.
    mmio_write(UART1_BASE + UART_FIFO, 0x42);
    let post_write_status = mmio_read(UART1_BASE + UART_STATUS);
    info!("After writing 1 byte to FIFO: STATUS=0x{:08X} (txfifo_cnt={})",
        post_write_status, (post_write_status >> 16) & 0xFF);

    let pattern: [u8; 16] = [
        0x00, 0xFF, 0xAA, 0x55, 0x01, 0x02, 0x04, 0x08,
        0x10, 0x20, 0x40, 0x80, 0x12, 0x34, 0x56, 0x78,
    ];

    info!("TX: {:02X?}", pattern);
    for &b in pattern.iter() {
        uart1_write_byte(b);
    }

    // 16 bytes @ 115200 baud * 10 bits/byte = ~1.4 ms total.
    // Spin a bit to let bits propagate through the wire and back into RX FIFO.
    esp32p4_hal_testing::busy_delay(2_000_000);

    let mut rx_buf = [0u8; 16];
    let mut total = 0;
    let mut spin_no_data = 0u32;
    while total < rx_buf.len() && spin_no_data < 1_000_000 {
        match uart1_read_byte_nb() {
            Some(b) => { rx_buf[total] = b; total += 1; spin_no_data = 0; }
            None => { spin_no_data += 1; }
        }
    }

    info!("RX: {:02X?} ({} of {} bytes)", &rx_buf[..total], total, pattern.len());

    let ok = total == pattern.len() && rx_buf == pattern;
    if ok {
        info!("=== test_uart_loopback: PASS ===");
        esp32p4_hal_testing::signal_pass();
    } else {
        info!("=== test_uart_loopback: FAIL ===");
    }

    esp32p4_hal_testing::park_alive("test_uart_loopback");
}
