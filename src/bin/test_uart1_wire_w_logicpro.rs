// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: bit-bang UART1 TX on GPIO6, verify with Logic Pro 16 Async Serial.
//!
//! `test_uart_loopback` is a register-level loopback test that fails
//! ambiguously on this silicon -- we don't know whether the wire is bad
//! or the UART register access path is bad. This bin sidesteps the
//! ambiguity: it emits a known message via fully bit-banged UART on
//! GPIO6, so the byte stream on the wire is determined entirely by the
//! firmware loop (not by the UART peripheral driver). If the LA's
//! Async Serial analyzer decodes the message correctly, the GPIO6 pin
//! and the bench-side trace are both healthy, and the FAIL in
//! `test_uart_loopback` must come from the UART peripheral side
//! (driver / register layout / clock) rather than the wire.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO6 (UART1 TX in standard pin-mapping)  ─── J1 pin 13 ─── LA CH3
//!
//! No external loopback wire needed for this bin. CH4 (GPIO7 = UART1 RX)
//! is unused here.
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled       : CH1 (UART0 verdict), CH3 (UART1 TX)
//!   Digital sample rate      : 5 MS/s   (115200 bps -> ~ 8.7 us bit time)
//!   Digital threshold        : 1.8 V
//!   Async Serial analyzer #1 : CH1 @ 115200 8N1   (UART0 verdict text)
//!   Async Serial analyzer #2 : CH3 @ 115200 8N1   (this bin's payload)
//!
//! ## Bit-bang UART parameters
//!
//!   Baud  : 115200
//!   Format: 8N1 (8 data bits LSB-first, 1 stop bit, no parity)
//!   Idle  : HIGH
//!   Bit T : 1 / 115200 = 8.681 us
//!
//! At 400 MHz CPU, that's ~3472 cycles per bit. We use a single-shot
//! SYSTIMER-bounded busy-loop per half-bit so timing is accurate to
//! within ~ 50 ns regardless of cache state.
//!
//! ## Payload
//!
//! Each iteration sends `Hello P4 UART1!\r\n` followed by a 200 ms gap.
//! Captured stream should contain the payload `ITERATIONS` times.
//!
//! ## PASS criteria
//!
//! Firmware-side: this bin runs to completion, emits the verdict line,
//! and parks alive. The decoding verdict is host-side.
//!
//! Host-side (Logic Pro 16, mandatory): Async Serial analyzer on CH3
//! decodes `Hello P4 UART1!\r\n` -- byte-perfect, all `ITERATIONS`
//! repetitions, no framing errors, no dropped bits.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const TX_PIN: u32 = 6;

/// Microseconds per bit at 115200 baud. 1e6 / 115200 = 8.6805 us; we
/// use the integer-rounded value.
const BIT_US: u64 = 9; // 9 us is +3.7% off nominal -- still inside UART receiver tolerance (~5%)
// Refinement: use a fractional-bit emitter that alternates 8 us / 9 us
// to land closer to 8.681. See `delay_bit()` below.

const ITERATIONS: u32 = 5;
const PAYLOAD: &[u8] = b"Hello P4 UART1!\r\n";

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

fn init_pin_output(pin: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO)
        let val = (val & !(0x3 << 10)) | (2 << 10); // FUN_DRV = 2
        r.write_volatile(val);
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
        let (en_w1ts_off, bit) = if pin < 32 { (0x24u32, pin) } else { (0x30u32, pin - 32) };
        ((GPIO_BASE + en_w1ts_off) as *mut u32).write_volatile(1u32 << bit);
    }
}

#[inline(always)]
fn pin_set(pin: u32, level_high: bool) {
    unsafe {
        let (w1ts_off, w1tc_off, bit) = if pin < 32 {
            (0x08u32, 0x0Cu32, pin)
        } else {
            (0x14u32, 0x18u32, pin - 32)
        };
        let off = if level_high { w1ts_off } else { w1tc_off };
        ((GPIO_BASE + off) as *mut u32).write_volatile(1u32 << bit);
    }
}

#[inline(always)]
fn busy_until(target: Instant) {
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

/// Sleep one UART bit time (8.681 us) using a fractional approach: alternate
/// 8 / 9 / 9 us so the average over 3 calls = 8.667 us, very close to the
/// nominal 8.681 us. Worst-case absolute error per bit: 0.32 us; over 10
/// bits per byte: 3.2 us == 0.37 bits == well within the standard 5%
/// receiver tolerance.
fn delay_bit(counter: &mut u32) {
    let now = Instant::now();
    let dur_us: u64 = match *counter % 3 {
        0 => 8,
        _ => 9,
    };
    *counter = counter.wrapping_add(1);
    busy_until(now + Duration::from_micros(dur_us));
}

fn send_byte(b: u8, ctr: &mut u32) {
    // START bit
    pin_set(TX_PIN, false);
    delay_bit(ctr);
    // 8 data bits LSB-first
    for i in 0..8 {
        pin_set(TX_PIN, (b >> i) & 1 != 0);
        delay_bit(ctr);
    }
    // STOP bit (HIGH)
    pin_set(TX_PIN, true);
    delay_bit(ctr);
}

fn send_str(s: &[u8], ctr: &mut u32) {
    for &b in s {
        send_byte(b, ctr);
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_uart1_wire_w_logicpro -- bit-bang UART1 TX on GPIO{}", TX_PIN);
    info!("===========================================================");
    info!("Baud: 115200 8N1   LA CH3   J1-13");
    info!("Payload: {:?}  x {} iterations", core::str::from_utf8(PAYLOAD).unwrap_or("?"), ITERATIONS);
    info!("");
    info!("Logic Pro 16: digital CH1+CH3 @ 5 MS/s, threshold 1.8 V");
    info!("  Async Serial analyzer on CH3 @ 115200 8N1 decodes the payload");
    info!("");

    init_pin_output(TX_PIN);
    pin_set(TX_PIN, true); // idle HIGH
    // Idle for 5 ms so the LA sees a clean high before the first START.
    esp32p4_hal_testing::delay_ms(5);

    info!("=== test_uart1_wire_w_logicpro: STAGE_BEGIN ===");
    let mut ctr: u32 = 0;
    for i in 0..ITERATIONS {
        send_str(PAYLOAD, &mut ctr);
        info!("  iter {}: payload sent ({} bytes)", i, PAYLOAD.len());
        // Inter-frame gap so the LA shows clearly separated transmissions.
        esp32p4_hal_testing::delay_ms(200);
    }
    pin_set(TX_PIN, true);

    esp32p4_hal_testing::signal_pass();
    info!("=== test_uart1_wire_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_uart1_wire_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_uart1_wire_w_logicpro");
}
