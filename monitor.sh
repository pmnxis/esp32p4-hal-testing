#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
#
# SPDX-License-Identifier: MIT OR Apache-2.0

# Standalone serial monitor for ESP32-P4 EV Board.
#
# Use when:
#   - run_tests.sh INCONCLUSIVE and you want raw output
#   - debugging panic/hang without re-flashing
#   - watching tests that loop forever (don't reach DONE)
#
# Usage:
#   ./monitor.sh                  # auto-detect port, 115200 baud
#   ./monitor.sh /dev/cu.usbmodem14101
#   ./monitor.sh -b 460800        # custom baud
#
# Press Ctrl+C to exit.

set -u
BAUD=115200
PORT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -b|--baud) BAUD="$2"; shift 2;;
        -h|--help)
            head -15 "$0" | tail -14
            exit 0;;
        /dev/*) PORT="$1"; shift;;
        *) echo "Unknown arg: $1" >&2; exit 1;;
    esac
done

# Auto-detect port if not specified
if [[ -z "$PORT" ]]; then
    case "$(uname)" in
        Darwin)
            PORT=$(ls /dev/cu.usbmodem* /dev/cu.usbserial* 2>/dev/null | head -1)
            ;;
        Linux)
            PORT=$(ls /dev/ttyACM* /dev/ttyUSB* 2>/dev/null | head -1)
            ;;
        *)
            echo "Unsupported OS: $(uname). Only macOS/Linux." >&2
            exit 1
            ;;
    esac
    if [[ -z "$PORT" ]]; then
        echo "No serial port detected. Specify one explicitly:" >&2
        echo "  ./monitor.sh /dev/ttyUSB0" >&2
        exit 1
    fi
fi

echo "Monitor: $PORT @ $BAUD baud"
echo "Press Ctrl+C to exit, Ctrl+T+R to reset (espflash monitor)"
echo "---"

# espflash monitor handles flow control + decoding
exec espflash monitor --baud "$BAUD" --port "$PORT"
