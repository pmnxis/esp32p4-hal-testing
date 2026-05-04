// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Test: bit-bang a single CAN classic standard frame on a GPIO,
//! decoded by Logic Pro 16's CAN analyzer.
//!
//! ESP-HAL's TWAI driver is gated `not_supported` on this PAC version
//! (PAC drift -- see `PERIPHERAL_TESTABILITY_w_logicpro.md`). We can
//! still verify CAN protocol generation at the bit level by hand-
//! shifting a CAN-classic frame onto a GPIO at the configured baud
//! and letting Logic 2's CAN Frame analyzer decode it.
//!
//! This is *not* a bus test (no CAN transceiver, no CAN_H/CAN_L
//! differential pair, no second node to ACK). It is a *protocol*
//! test on the logic-level TXD line that a CAN controller would have
//! produced. Logic 2's CAN analyzer can be set to "Single Ended"
//! mode and read the wire just like the controller's TXD pin.
//!
//! ## Frame layout (CAN classic, standard 11-bit ID)
//!
//!   SOF (1 dominant 0)
//!   ID  (11 bits)            -- 0x123 (binary 100100011)
//!   RTR (1 bit, 0 = data)
//!   IDE (1 bit, 0 = standard)
//!   r0  (1 reserved 0)
//!   DLC (4 bits)             -- 4 data bytes
//!   DATA (4 bytes "ESP4")    -- 0x45 0x53 0x50 0x34
//!   CRC (15 bits)            -- computed
//!   CRC delimiter (1 recessive 1)
//!   ACK slot (1 bit, recessive 1 -- no receiver to acknowledge)
//!   ACK delimiter (1 recessive 1)
//!   EOF (7 recessive 1s)
//!
//! Bit stuffing is applied from SOF through CRC sequence. A complement
//! bit is inserted after any run of 5 same-value bits.
//!
//! ## Wiring (la_channel_map.csv)
//!
//!   GPIO33 -> LA CH9 -> J1 pin 31
//!
//! Idle line is recessive (HIGH). Pin starts HIGH; the SOF dominant
//! bit drops it LOW.
//!
//! ## Logic Pro 16 setup
//!
//!   Digital ch enabled : CH1, CH9
//!   Sample rate        : 5 MS/s   (covers 125 kbit/s with 40x
//!                                  oversampling per bit)
//!   Threshold          : 1.8 V
//!   Capture            : >= 4 s
//!   CAN analyzer       : input CH9, baud 125000, format "Single Ended
//!                        (Logic Level)" / "TX side"
//!
//! ## PASS criteria
//!
//! Firmware-side: bin runs all iterations, prints PASS marker.
//!
//! Host-side: CAN analyzer decodes
//!   ID = 0x123, DLC = 4, DATA = [0x45, 0x53, 0x50, 0x34], CRC OK
//! at least 5 times (one per iteration).

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;
use esp_hal::time::{Duration, Instant};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

const TX_PIN: u32 = 33;

/// 125 kbit/s -> 8 us per bit.
const BIT_US: u64 = 8;

const ITERATIONS: u32 = 5;

const FRAME_ID: u16 = 0x123;
const FRAME_DATA: &[u8] = b"ESP4";
const FRAME_DLC: u8 = 4;

#[inline(always)]
fn iomux_reg(pin: u32) -> *mut u32 {
    (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32
}

fn init_pin_output(pin: u32) {
    unsafe {
        let r = iomux_reg(pin);
        let val = r.read_volatile();
        let val = (val & !(0x7 << 12)) | (1 << 12);
        let val = (val & !(0x3 << 10)) | (2 << 10);
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

/// CAN-15 CRC: poly 0x4599. Applied to the bit sequence from SOF
/// through DATA (before stuffing).
fn can_crc15(bits: &[bool]) -> u16 {
    let mut crc: u16 = 0;
    for &b in bits {
        let xor = (crc >> 14) as u16 ^ (b as u16);
        crc <<= 1;
        crc &= 0x7FFF;
        if xor & 1 != 0 {
            crc ^= 0x4599;
        }
    }
    crc & 0x7FFF
}

/// Build the un-stuffed bit sequence (logical TX side, dominant=0,
/// recessive=1) for a single standard data frame.
fn build_unstuffed(out: &mut heapless::Vec<bool, 128>) {
    // SOF (dominant 0)
    out.push(false).unwrap();
    // ID (11 bits, MSB first)
    for i in (0..11).rev() {
        out.push((FRAME_ID >> i) & 1 != 0).unwrap();
    }
    // RTR = 0 (data frame)
    out.push(false).unwrap();
    // IDE = 0 (standard)
    out.push(false).unwrap();
    // r0 reserved = 0
    out.push(false).unwrap();
    // DLC (4 bits, MSB first)
    for i in (0..4).rev() {
        out.push((FRAME_DLC >> i) & 1 != 0).unwrap();
    }
    // DATA
    for &b in FRAME_DATA {
        for i in (0..8).rev() {
            out.push((b >> i) & 1 != 0).unwrap();
        }
    }
    // CRC over what we've emitted so far (SOF..DATA)
    let crc = can_crc15(out.as_slice());
    for i in (0..15).rev() {
        out.push((crc >> i) & 1 != 0).unwrap();
    }
}

/// Apply bit stuffing: after 5 consecutive same-value bits in the
/// stuffable region, insert one complement bit. Stuffing applies to
/// SOF through CRC sequence (everything pushed by `build_unstuffed`).
fn apply_stuffing(input: &[bool], out: &mut heapless::Vec<bool, 192>) {
    let mut last = !input[0]; // ensure first bit triggers no run
    let mut run = 0u8;
    for &b in input {
        if b == last {
            run += 1;
        } else {
            run = 1;
            last = b;
        }
        out.push(b).unwrap();
        if run == 5 {
            // insert complement
            out.push(!b).unwrap();
            last = !b;
            run = 1;
        }
    }
}

fn emit_frame() {
    let mut unstuffed: heapless::Vec<bool, 128> = heapless::Vec::new();
    build_unstuffed(&mut unstuffed);

    let mut stuffed: heapless::Vec<bool, 192> = heapless::Vec::new();
    apply_stuffing(unstuffed.as_slice(), &mut stuffed);

    // Append CRC delimiter, ACK slot, ACK delimiter, EOF (10 recessive bits).
    // Stuffing does NOT apply from CRC delimiter onwards.
    for _ in 0..10 {
        stuffed.push(true).unwrap();
    }

    let t0 = Instant::now();
    for (i, &bit) in stuffed.iter().enumerate() {
        // CAN dominant = 0V on logic-level TX, recessive = 3V3 on TX.
        // We're driving a single-ended logic line, so:
        pin_set(TX_PIN, bit);
        let target = t0 + Duration::from_micros(BIT_US * (i as u64 + 1));
        busy_until(target);
    }
    pin_set(TX_PIN, true); // idle recessive
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("===========================================================");
    info!(" test_twai_canframe_w_logicpro -- bit-bang CAN classic frame");
    info!("===========================================================");
    info!("Pin: GPIO{} -> LA CH9 (J1-31)", TX_PIN);
    info!("Frame: ID=0x{:03X} DLC={} DATA={:?}",
          FRAME_ID, FRAME_DLC, core::str::from_utf8(FRAME_DATA).unwrap_or("?"));
    info!("Baud: 125 kbit/s   (8 us per bit)");
    info!("");
    info!("Logic Pro 16: digital CH1+9 @ 5 MS/s, threshold 1.8 V");
    info!("  CAN analyzer on CH9: 125 kbit/s, single-ended TX side");
    info!("");

    init_pin_output(TX_PIN);
    pin_set(TX_PIN, true); // idle recessive
    esp32p4_hal_testing::delay_ms(20);

    info!("=== test_twai_canframe_w_logicpro: STAGE_BEGIN ===");
    for i in 0..ITERATIONS {
        emit_frame();
        info!("  iter {}: frame emitted", i);
        esp32p4_hal_testing::delay_ms(200);
    }

    esp32p4_hal_testing::signal_pass();
    info!("=== test_twai_canframe_w_logicpro: PASS (verify on Logic Pro 16) ===");
    info!("=== test_twai_canframe_w_logicpro: DONE ===");
    esp32p4_hal_testing::park_alive("test_twai_canframe_w_logicpro");
}
