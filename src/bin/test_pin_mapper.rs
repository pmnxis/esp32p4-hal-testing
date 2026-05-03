// SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tool: identify which Logic Pro 16 channel is wired to which GPIO.
//!
//! Use case: CH0 = RST, CH1 = GPIO23 are confirmed. Channels CH2..CH15 are
//! unknown because the harness was re-clipped during bench setup. Run this
//! binary, capture ~3 s on the LA, then count pulses on each channel.
//!
//! Encoding: each candidate GPIO emits N pulses per cycle, where N = its
//! GPIO number. All pins start each cycle in lockstep at t=0 and finish at
//! their own (2*N)-th tick. A long inter-cycle gap makes the boundary
//! unmistakable.
//!
//! Reading the capture:
//!   - On any LA channel, count rising edges within one cycle.
//!   - Pulse count == GPIO number for that channel.
//!   - Example: CH8 shows 22 rising edges -> CH8 is GPIO22.
//!
//! Sanity heartbeat before the loop starts: GPIO23 blinks 3 times slowly,
//! so you can confirm CH1 = GPIO23 first (and that the firmware is alive).
//!
//! Pulse: 5 ms HIGH + 5 ms LOW = 10 ms each.
//! Max pin in CANDIDATES: 54 -> 540 ms + 500 ms gap = ~1.04 s per cycle.
//!
//! Also logs each cycle # over UART0 console (GPIO37 TX) so an LA tap on
//! CH1 alternative or a serial monitor can correlate timing.
//!
//! NC pins observed so far on V1.7 EV board: GPIO0, GPIO1 (R series 0R NC).
//! Edit CANDIDATES if you confirm more NC pins from the schematic.
//!
//! WARNING: per `.investigation/la_channel_map.csv` the bench has two
//! pairs of physically shorted pins:
//!     GPIO20 <-> GPIO22  (group A pseudo-DAC self-loop)
//!     GPIO48 <-> GPIO53  (group B pseudo-DAC self-loop)
//! This bin drives both endpoints of each pair as push-pull GPIO outputs
//! while sweeping. They never *fight* each other because each cycle each
//! pin emits its own pulse train at staggered times -- but they DO share
//! the wire, so the LA capture will see the OR of two pulse trains on
//! either channel of the pair. Pulse-count decode still works (each pin
//! emits its own GPIO-number worth of pulses, so the per-channel total
//! is `gpio_a + gpio_b`); take that into account when decoding the LA.
//! If you want clean per-pin pulse trains, drop one pin from each pair
//! in CANDIDATES.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf as _;

esp_bootloader_esp_idf::esp_app_desc!();
use log::info;

const GPIO_BASE: u32 = 0x500E_0000;
const IO_MUX_BASE: u32 = 0x500E_1000;

// Header-exposed GPIO candidates per J1 schematic (ESP32-P4X EV V1.7).
// Excludes:
//   - GPIO0, GPIO1 (NC: R series 0R not stuffed)
//   - GPIO37, 38 (UART0 console used by esp-println; reconfiguring them
//     would disconnect the log path that the host monitor uses)
//   - USB-Serial-JTAG (24, 25)
// Tweak to match your wiring after each pass.
const CANDIDATES: &[u32] = &[
    2, 3, 4, 5, 6, 7, 8,
    20, 21, 22, 23,
    26, 27, 32, 33, 36,
    45, 46, 47, 48, 53, 54,
];

// === GPIO bank 0 (pins 0-31) and bank 1 (pins 32-54) register helpers ===
// Offsets per esp32p4 PAC, as documented in lib.rs.

fn iomux_select_gpio(pin: u32) {
    unsafe {
        // ESP32-P4: IO_MUX has a 4-byte reserved slot before pin[0],
        // so pin N is at BASE + 0x04 + N*4. See lib.rs::init_led.
        let iomux = (IO_MUX_BASE + 0x04 + pin * 4) as *mut u32;
        let v = iomux.read_volatile();
        let v = (v & !(0x7 << 12)) | (1 << 12); // MCU_SEL = 1 (GPIO)
        let v = (v & !(0x3 << 10)) | (2 << 10); // drive strength = 2
        iomux.write_volatile(v);
    }
}

fn func_out_sel_gpio(pin: u32) {
    unsafe {
        // OUT_SEL = 256 (drive from GPIO_OUT_REG), OEN_SEL = 1 (use ENABLE_REG).
        ((GPIO_BASE + 0x558 + pin * 4) as *mut u32).write_volatile(0x100 | (1 << 10));
    }
}

fn enable_output(pin: u32) {
    unsafe {
        if pin < 32 {
            // Bank 0: GPIO_ENABLE_W1TS = 0x24 (per lib.rs PAC reference).
            ((GPIO_BASE + 0x24) as *mut u32).write_volatile(1 << pin);
        } else {
            // Bank 1: GPIO_ENABLE1_W1TS = 0x30 (extrapolated from bank 0 +0x10).
            ((GPIO_BASE + 0x30) as *mut u32).write_volatile(1 << (pin - 32));
        }
    }
}

fn gpio_init_output(pin: u32) {
    iomux_select_gpio(pin);
    func_out_sel_gpio(pin);
    enable_output(pin);
}

fn gpio_high(pin: u32) {
    unsafe {
        if pin < 32 {
            ((GPIO_BASE + 0x08) as *mut u32).write_volatile(1 << pin);
        } else {
            ((GPIO_BASE + 0x14) as *mut u32).write_volatile(1 << (pin - 32));
        }
    }
}

fn gpio_low(pin: u32) {
    unsafe {
        if pin < 32 {
            ((GPIO_BASE + 0x0C) as *mut u32).write_volatile(1 << pin);
        } else {
            ((GPIO_BASE + 0x18) as *mut u32).write_volatile(1 << (pin - 32));
        }
    }
}

#[esp_hal::esp_riscv_rt::entry]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!("=== test_pin_mapper: LA channel discovery ===");
    info!("each GPIO emits its number as pulse count per cycle");
    info!("candidates: {:?}", CANDIDATES);

    // Initialize all candidates as outputs, low.
    for &p in CANDIDATES {
        gpio_init_output(p);
        gpio_low(p);
    }

    // Heartbeat on GPIO23 (LA CH1): 3 slow blinks. Lets you confirm CH1 = LED
    // before the discovery loop starts emitting overlapping pulse trains.
    info!("heartbeat on GPIO23 (CH1) x3");
    for _ in 0..3 {
        gpio_high(23);
        esp32p4_hal_testing::delay_ms(150);
        gpio_low(23);
        esp32p4_hal_testing::delay_ms(150);
    }
    esp32p4_hal_testing::delay_ms(800);

    info!("starting discovery loop -- count pulses per cycle on each LA ch");

    // Find the largest pin number; cycle length = 2 * max ticks (HIGH + LOW per pulse).
    let max_n: u32 = CANDIDATES.iter().copied().max().unwrap_or(54);
    let total_ticks: u32 = max_n * 2;

    let mut cycle: u32 = 0;
    loop {
        info!("cycle #{}", cycle);

        // Lockstep tick loop. At each tick (5 ms):
        //   for pin p: if tick < 2*p, drive HIGH on even ticks / LOW on odd ticks.
        //              else hold low.
        for tick in 0..total_ticks {
            for &p in CANDIDATES {
                if tick < p * 2 {
                    if tick & 1 == 0 {
                        gpio_high(p);
                    } else {
                        gpio_low(p);
                    }
                } else {
                    gpio_low(p);
                }
            }
            esp32p4_hal_testing::delay_ms(5);
        }

        // Inter-cycle gap. All candidates held low. Cycle boundary unmistakable.
        for &p in CANDIDATES {
            gpio_low(p);
        }
        esp32p4_hal_testing::delay_ms(500);

        cycle = cycle.wrapping_add(1);
    }
}
