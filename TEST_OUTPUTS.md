<!--
SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)

SPDX-License-Identifier: MIT OR Apache-2.0
-->

# ESP32-P4 Test Binaries -- Expected Outputs

Each test binary in `src/bin/` outputs via UART0 (115200 8N1) at GPIO37/38.
Success signals: LED blinks rapidly (10x on GPIO23), log contains `PASS`/`OK` + `DONE`.
Failure signals: LED steady on + halt, log contains `FAIL`/`mismatch`/`ERROR`.

## Log Conventions

- Start line: `=== test_name: <description> ===`
- End line:   `=== test_name: DONE ===`
- Pass marker: `PASS`, `OK`, `CONFIRMED`, `transferred OK`
- Fail marker: `FAIL`, `mismatch`, `ERROR`, `timeout!`

---

## Basic Verification Tests

### test_init
**Purpose:** Minimal esp-hal init + SYSTIMER + LED blink.
**Required HW:** None (any EV Board).

Expected output:
```
=== test_init: ESP32-P4 HAL basic init ===
esp-hal init OK
Chip revision: v3.x
SYSTIMER check: counter advancing
LED blink start (GPIO23)
=== test_init: DONE ===
```
**Pass criteria:** `DONE` printed + LED blinks 10x.

---

### test_chip_id
**Purpose:** Chip revision + eFuse ID readout.
**Required HW:** None.

Expected output:
```
=== test_chip_id: Chip identification ===
Chip revision (esp-hal): v3.0              # or v3.1
eFuse raw: RD_MAC_SPI_SYS_2 = 0x00000xxx
  rev[5:0] = 0b00000x (x)
  rev[23]  = 0 or 1
Marking code: F (eco5) or G (default v3.1)
Espressif OUI: YES
CPU mvendorid: 0x5E00xxxx
CPU marchid:   0x6000xxxx
=== test_chip_id: DONE ===
```
**Pass criteria:** Marking code is `F` or `G`, mvendorid non-zero.

---

### test_efuse
**Purpose:** eFuse readout -- MAC address + chip revision.
**Required HW:** None.

Expected output:
```
=== test_efuse: eFuse readout ===
MAC address: xx:xx:xx:xx:xx:xx              # Espressif OUI: first byte 0x30..0x98
WAFER_VERSION_MAJOR: 3
WAFER_VERSION_MINOR: 0 or 1
BLK_VERSION_MINOR: x
=== test_efuse: DONE ===
```
**Pass criteria:** MAC address non-zero, WAFER_VERSION_MAJOR == 3.

---

### test_clock
**Purpose:** Clock tree verification (SYSTIMER + CPU freq).
**Required HW:** None.

Expected output:
```
=== test_clock: Clock tree verification ===
SYSTIMER tick: 16 MHz verified
CPU frequency: 400 MHz (or configured)
APB frequency: 100 MHz
=== test_clock: DONE ===
```
**Pass criteria:** SYSTIMER advances at 16MHz +/- 1%.

---

### test_gpio
**Purpose:** Basic GPIO output + input readback.
**Required HW:** None (internal loopback via IO MUX).

Expected output:
```
=== test_gpio: GPIO output/input ===
GPIO23 output HIGH -> readback: 1
GPIO23 output LOW  -> readback: 0
=== test_gpio: DONE ===
```
**Pass criteria:** Readback matches set value.

---

### test_uart_loopback
**Purpose:** UART1 internal TX<->RX loopback via GPIO matrix.
**Required HW:** None (uses internal signal routing).

Expected output:
```
=== test_uart_loopback: UART1 internal loopback ===
Sent 16 bytes, received 16 bytes
Data: OK
=== test_uart_loopback: DONE ===
```
**Pass criteria:** Sent/received data match.

---

### test_i2c_scan
**Purpose:** I2C0 bus scan (prints any device addresses found).
**Required HW:** Optional I2C devices on SDA/SCL pins.

Expected output (no devices):
```
=== test_i2c_scan: I2C bus scan ===
Scanning addresses 0x08 .. 0x77...
(no devices found)
=== test_i2c_scan: DONE ===
```
Expected output (with BME280 @ 0x76):
```
Device found at 0x76
```
**Pass criteria:** Scan completes without I2C bus hang.

---

### test_emac_phy
**Purpose:** EMAC PHY detection via MDIO.
**Required HW:** EV Board with IP101GRI PHY populated (no Ethernet cable needed).

Expected output:
```
=== test_emac_phy: PHY detection via MDIO ===
MDIO read PHY 0x01 reg 0x02 (PHY ID1): 0x0243   # IP101GRI
MDIO read PHY 0x01 reg 0x03 (PHY ID2): 0x0C54
PHY detected: IP101GRI
=== test_emac_phy: DONE ===
```
**Pass criteria:** PHY ID1=0x0243, ID2=0x0C54 (IP101GRI signature).

---

### test_usb_dwc2
**Purpose:** USB OTG HS DWC2 presence check via GSNPSID.
**Required HW:** None.

Expected output:
```
=== test_usb_dwc2: USB OTG HS controller check ===
DWC2 GSNPSID: 0x4F54430A                    # Synopsys DWC OTG v4.30a
DWC2 present: YES
=== test_usb_dwc2: DONE ===
```
**Pass criteria:** GSNPSID high 16 bits == 0x4F54 (ASCII "OT").

---

### test_crypto
**Purpose:** AES + SHA hardware accelerator with known vectors.
**Required HW:** None.

Expected output:
```
=== test_crypto: AES + SHA hardware accelerator ===
AES-128 ECB test vector: PASS
SHA-256 "abc" -> 0xba7816bf..........        # NIST test vector
SHA matches expected: PASS
=== test_crypto: DONE ===
```
**Pass criteria:** Both AES and SHA match NIST test vectors.

---

### test_psram
**Purpose:** PSRAM initialization + basic read/write at 0x4800_0000.
**Required HW:** EV Board with AP HEX PSRAM populated.

Expected output:
```
=== test_psram: PSRAM initialization ===
MPLL configured: 400 MHz
PSRAM MSPI controller init OK
MMU mapping: 512 pages x 64KB = 32 MB
Write/read pattern at 0x48000000: PASS
=== test_psram: DONE ===
```
**Pass criteria:** Write/read pattern match (no stuck-at or shifted values).

---

## Corner Case Tests

### test_systimer_corner
**Purpose:** SYSTIMER edge cases -- monotonicity, rollover.
Expected: `Monotonic: PASS`, `Rollover behavior verified`.

### test_gpio_all_pins
**Purpose:** Sweep all 55 GPIOs output/readback.
Expected: `All 55 pins OK` (or list of pins with issues).

### test_memory_boundary
**Purpose:** SRAM edge-case access (alignment, boundary crossing).
Expected: `All boundary accesses OK`.

### test_clock_switch
**Purpose:** Switch CPU clock source XTAL <-> CPLL dynamically.
Expected: Correct freq before/after switch.

### test_interrupt
**Purpose:** CLIC interrupt delivery (software-triggered).
Expected: Handler ran, correct interrupt ID captured.

### test_wdt
**Purpose:** Watchdog feed + controlled timeout.
Expected: Feed works, eventual WDT reset reason detected.

### test_crypto_rng_dma
**Purpose:** AES + RNG + DMA combined exercise.
Expected: RNG entropy sample valid, DMA transfer OK.

### test_sram_psram_crossover
**Purpose:** Copy data between SRAM and PSRAM, verify integrity.
Expected: `All transfers match` (requires test_psram to pass first).

---

## Errata Tests

### test_errata (master)
**Purpose:** Enumerate chip errata status for current revision.
Expected output:
```
=== test_errata: ESP32-P4 errata verification ===
Chip revision: v3.0 or v3.1
v3.0 active errata: MSPI-749, MSPI-750, MSPI-751, ROM-764, Analog-765, DMA-767, APM-560
v3.1 active errata: ROM-770 (no fix)
Fixed in v3.0+: RMT-176, I2C-308 verified
=== test_errata: DONE ===
```

### test_errata_mspi749
**Purpose:** MSPI-749 boot fault -- check reset reason.
Expected (v3.0): Possibly `WDT reset detected -- MSPI-749 recovery path`.
Expected (v3.1): `Clean power-on boot (fix confirmed)`.

### test_errata_mspi750
**Purpose:** MSPI-750 PSRAM unaligned DMA stale data.
**Required:** test_psram must pass first.
Expected (v3.0): `Unaligned DMA returns stale data -- errata active`.
Expected (v3.1): `No stale data -- fix confirmed`.

### test_errata_mspi751
**Purpose:** MSPI-751 PSRAM clock ratio data errors (10K stress test).
**Required:** PSRAM active.
Expected (v3.0): May report errors if clock ratio violates constraint.
Expected (v3.1): `0 errors in 10000 iterations`.

### test_errata_dma767
**Purpose:** DMA CH0 transaction ID overlap (register access sanity).
Expected: CH0/1/2 all register-accessible; recommendation to use CH1/CH2 on v3.0.

### test_errata_apm560
**Purpose:** APM-560 unauthorized access stall (safe register read only).
Expected: APM regs accessible, no intentional unauthorized access attempted.

### test_dma_mem2mem
**Purpose:** Full DMA mem2mem transfer on CH0, CH1, CH2 with descriptor setup.
Expected:
```
CH0: 256 bytes transferred OK         # may FAIL on v3.0 due to DMA-767
CH1: 256 bytes transferred OK
CH2: 256 bytes transferred OK
All 3 channels: PASS
```

---

## Test Suite Summary

| Group    | Count | Typical Duration | HW Dependency               |
|----------|-------|------------------|-----------------------------|
| Basic    | 12    | ~2 min           | EV Board only               |
| Corner   | 7     | ~3 min           | EV Board (some need PSRAM)  |
| Errata   | 7     | ~2 min           | EV Board, v3.0 vs v3.1 aware|

Run via `./run_tests.sh --basic|--corner|--errata` or individually.
Logs auto-saved to `/tmp/<test_name>.log`.
