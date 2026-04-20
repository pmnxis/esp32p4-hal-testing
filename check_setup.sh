#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
#
# SPDX-License-Identifier: MIT OR Apache-2.0

# Pre-flight check for ESP32-P4 HW verification.
#
# Run this BEFORE plugging in the EV Board to verify host environment.
# Catches setup issues early so you don't waste time debugging the board.
#
# Usage: ./check_setup.sh

set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

OK_CNT=0
FAIL_CNT=0
WARN_CNT=0

check() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo -e "  ${GREEN}[OK]${NC}    $name"
        OK_CNT=$((OK_CNT+1))
        return 0
    else
        echo -e "  ${RED}[FAIL]${NC}  $name"
        FAIL_CNT=$((FAIL_CNT+1))
        return 1
    fi
}

warn_check() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo -e "  ${GREEN}[OK]${NC}    $name"
        OK_CNT=$((OK_CNT+1))
        return 0
    else
        echo -e "  ${YELLOW}[WARN]${NC}  $name"
        WARN_CNT=$((WARN_CNT+1))
        return 1
    fi
}

echo "=== Pre-flight check for ESP32-P4 HW verification ==="
echo

echo "--- Toolchain ---"
check "rustup installed" command -v rustup
check "nightly-2026-04-11 toolchain installed" \
    bash -c "rustup toolchain list | grep -q nightly-2026-04-11"
check "rust-src component" \
    bash -c "rustup component list --toolchain nightly-2026-04-11 --installed | grep -q rust-src"
check "riscv32imafc target installed" \
    bash -c "rustup target list --installed --toolchain nightly-2026-04-11 | grep -q riscv32imafc-unknown-none-elf"

echo
echo "--- Tools ---"
check "espflash >= 3.x" \
    bash -c "espflash --version | grep -qE 'espflash 3\.'"
warn_check "git available (for submodule sync)" command -v git

echo
echo "--- Repository state ---"
check "Cargo.toml present" test -f Cargo.toml
check ".cargo/config.toml present" test -f .cargo/config.toml
check "src/bin/ has test binaries" \
    bash -c "ls src/bin/test_*.rs >/dev/null"
check "esp-hal P4 metadata present" \
    test -f ../embedded-lib/esp-hal/esp-metadata-generated/src/_build_script_utils.rs

echo
echo "--- Build sanity (dry-run) ---"
echo "  Building test_init (smallest binary)..."
if cargo +nightly build --release --bin test_init 2>&1 | tail -3 | grep -q "Finished"; then
    echo -e "  ${GREEN}[OK]${NC}    test_init builds"
    OK_CNT=$((OK_CNT+1))
else
    echo -e "  ${RED}[FAIL]${NC}  test_init build failed -- run 'cargo +nightly build --bin test_init' for details"
    FAIL_CNT=$((FAIL_CNT+1))
fi

echo
echo "--- Serial port ---"
PORTS=""
case "$(uname)" in
    Darwin)
        PORTS=$(ls /dev/cu.usbmodem* /dev/cu.usbserial* /dev/cu.SLAB_USBtoUART* 2>/dev/null)
        ;;
    Linux)
        PORTS=$(ls /dev/ttyACM* /dev/ttyUSB* 2>/dev/null)
        ;;
    *)
        echo -e "  ${RED}[FAIL]${NC}  Unsupported OS: $(uname). Only macOS/Linux supported."
        FAIL_CNT=$((FAIL_CNT+1))
        ;;
esac
if [[ -n "$PORTS" ]]; then
    echo -e "  ${GREEN}[OK]${NC}    Serial ports detected:"
    echo "$PORTS" | sed 's/^/             /'
else
    echo -e "  ${YELLOW}[WARN]${NC}  No serial ports detected. Plug in the EV Board USB."
    WARN_CNT=$((WARN_CNT+1))
fi

# espflash board-info will only succeed if board is connected
if [[ -n "$PORTS" ]]; then
    echo
    echo "--- Board detection (espflash) ---"
    if BOARD_INFO=$(espflash board-info 2>&1); then
        echo -e "  ${GREEN}[OK]${NC}    Board detected:"
        echo "$BOARD_INFO" | grep -E "Chip type|Crystal|MAC" | sed 's/^/             /'
        OK_CNT=$((OK_CNT+1))
        if echo "$BOARD_INFO" | grep -q "esp32p4"; then
            echo -e "  ${GREEN}[OK]${NC}    Chip type is esp32p4"
        else
            echo -e "  ${YELLOW}[WARN]${NC}  Chip is not esp32p4 (got: $(echo "$BOARD_INFO" | grep "Chip type"))"
            WARN_CNT=$((WARN_CNT+1))
        fi
    else
        echo -e "  ${YELLOW}[WARN]${NC}  espflash board-info failed (board may be in a bad state):"
        echo "$BOARD_INFO" | head -3 | sed 's/^/             /'
        WARN_CNT=$((WARN_CNT+1))
    fi
fi

echo
echo "=== Summary ==="
echo -e "  ${GREEN}OK:    $OK_CNT${NC}"
echo -e "  ${YELLOW}WARN:  $WARN_CNT${NC}"
echo -e "  ${RED}FAIL:  $FAIL_CNT${NC}"
echo

if [[ $FAIL_CNT -gt 0 ]]; then
    echo -e "${RED}Setup has FAIL items. Fix before proceeding to HW verification.${NC}"
    exit 1
elif [[ $WARN_CNT -gt 0 ]]; then
    echo -e "${YELLOW}Setup has WARN items. Likely OK to proceed, but read above.${NC}"
    exit 0
else
    echo -e "${GREEN}Setup ready. Proceed with: ./run_tests.sh test_init${NC}"
    exit 0
fi
