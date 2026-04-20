<!--
SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)

SPDX-License-Identifier: MIT OR Apache-2.0
-->

# esp32p4-hal-testing

Small `#[no_std]` firmware binaries for testing the ESP32-P4 port of
[`pmnxis/esp-hal`](https://github.com/pmnxis/esp-hal) (branch
`pmnxis/esp32p4x`) on real hardware.

One binary per subsystem: init, clock, GPIO, UART, I2C, DMA, crypto,
PSRAM, EMAC, USB, errata-specific tests.

## Hardware

- **ESP32-P4X Function EV Board V1.7** (silicon v3.2 / ECO7).
- Optional: ESP-PROG on its 6-pin PROG port, wired to the board's
  BOOT / RESET button nets, for auto-reset during flashing.

## Build one test

```sh
cargo +nightly build --release --bin test_init
```

## Flash + run one test

```sh
cargo +nightly run --release --bin test_init
```

`cargo run` uses `espflash` via the runner in `.cargo/config.toml` and
opens the serial monitor automatically.

## Helper scripts

| Script | Purpose |
| ------ | ------- |
| `build_all.sh` | Build every `src/bin/*.rs` binary in one pass. |
| `run_tests.sh` | Flash each binary in turn and check its log for pass / fail markers. |
| `check_setup.sh` | Verify host tooling (Rust nightly, target, `espflash`). Does not need the board. |
| `monitor.sh` | Raw serial monitor on the default port. Useful for debugging. |

## What a test looks like

Each test follows a fixed log convention:

- Start line: `=== <name>: <description> ===`
- End line: `=== <name>: DONE ===`
- Pass markers: `PASS`, `OK`, `CONFIRMED`
- Fail markers: `FAIL`, `mismatch`, `ERROR`

An LED on GPIO23 also blinks on pass and stays on for fail.

Expected console output for every binary is recorded in
[`TEST_OUTPUTS.md`](TEST_OUTPUTS.md).

## License

Dual-licensed `MIT OR Apache-2.0`. See `LICENSES/` and `REUSE.toml`.
