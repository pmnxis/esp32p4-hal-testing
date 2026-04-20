#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
#
# SPDX-License-Identifier: MIT OR Apache-2.0

# ESP32-P4 HAL test runner
#
# Flashes each test binary in sequence, captures serial output, and reports
# pass/fail based on log patterns.
#
# Usage:
#   ./run_tests.sh                # run all tests
#   ./run_tests.sh test_init      # run single test
#   ./run_tests.sh --list         # list available tests
#   ./run_tests.sh --basic        # run only basic verification tests
#   ./run_tests.sh --errata       # run only errata tests
#   ./run_tests.sh --corner       # run only corner-case tests
#
# Environment:
#   ESPFLASH_PORT -- serial port (default: auto-detect via espflash)
#   TIMEOUT_SECS  -- per-test timeout (default: 10)

set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

TIMEOUT_SECS="${TIMEOUT_SECS:-10}"
PORT_ARG=""
if [[ -n "${ESPFLASH_PORT:-}" ]]; then
    PORT_ARG="--port $ESPFLASH_PORT"
fi

# Test groupings
BASIC_TESTS=(
    test_init
    test_chip_id
    test_efuse
    test_clock
    test_gpio
    test_systimer_corner
    test_uart_loopback
    test_i2c_scan
    test_emac_phy
    test_usb_dwc2
    test_crypto
    test_psram
)
CORNER_TESTS=(
    test_gpio_all_pins
    test_memory_boundary
    test_clock_switch
    test_interrupt
    test_wdt
    test_crypto_rng_dma
    test_sram_psram_crossover
)
ERRATA_TESTS=(
    test_errata
    test_errata_mspi749
    test_errata_mspi750
    test_errata_mspi751
    test_errata_dma767
    test_errata_apm560
    test_dma_mem2mem
)

# Color codes
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

list_tests() {
    echo "Basic verification:"
    printf "  %s\n" "${BASIC_TESTS[@]}"
    echo "Corner cases:"
    printf "  %s\n" "${CORNER_TESTS[@]}"
    echo "Errata tests:"
    printf "  %s\n" "${ERRATA_TESTS[@]}"
}

run_one() {
    local test_name="$1"
    local logfile="/tmp/${test_name}.log"

    echo -e "${YELLOW}==> ${test_name}${NC}"

    # Flash + monitor with timeout
    # espflash exits when monitor receives EOF or on Ctrl+C, so we rely on timeout
    timeout "${TIMEOUT_SECS}s" \
        cargo +nightly run --release --bin "${test_name}" $PORT_ARG \
        > "$logfile" 2>&1
    local rc=$?

    # Detect compile failure first (would mask serial output analysis)
    if grep -qE "^error\[E[0-9]+\]:|could not compile" "$logfile"; then
        echo -e "  ${RED}BUILD FAIL${NC} (log: $logfile)"
        grep -E "^error" "$logfile" | head -3 | sed 's/^/    /'
        echo "    Hint: run ./build_all.sh to verify compilation across all bins"
        return 1
    fi

    # Analyze log
    if grep -q "DONE" "$logfile" && grep -qE "PASS|OK|transferred OK" "$logfile" && ! grep -qE "FAIL|mismatch|ERROR|timeout!" "$logfile"; then
        echo -e "  ${GREEN}PASS${NC} (log: $logfile)"
        return 0
    elif grep -qE "FAIL|mismatch|ERROR" "$logfile"; then
        echo -e "  ${RED}FAIL${NC} (log: $logfile)"
        grep -E "FAIL|mismatch|ERROR" "$logfile" | head -5 | sed 's/^/    /'
        return 1
    else
        echo -e "  ${YELLOW}INCONCLUSIVE${NC} (rc=$rc, log: $logfile)"
        tail -5 "$logfile" | sed 's/^/    /'
        return 2
    fi
}

run_group() {
    local group_name="$1"; shift
    local tests=("$@")
    local pass=0 fail=0 incon=0

    echo "=== ${group_name} (${#tests[@]} tests) ==="
    for t in "${tests[@]}"; do
        run_one "$t"
        case $? in
            0) pass=$((pass+1));;
            1) fail=$((fail+1));;
            2) incon=$((incon+1));;
        esac
    done
    echo "=== ${group_name}: ${GREEN}${pass} PASS${NC}, ${RED}${fail} FAIL${NC}, ${YELLOW}${incon} INCONCLUSIVE${NC} ==="
}

case "${1:-}" in
    --list|-l)
        list_tests
        ;;
    --basic)
        run_group "Basic" "${BASIC_TESTS[@]}"
        ;;
    --corner)
        run_group "Corner" "${CORNER_TESTS[@]}"
        ;;
    --errata)
        run_group "Errata" "${ERRATA_TESTS[@]}"
        ;;
    "")
        run_group "Basic" "${BASIC_TESTS[@]}"
        run_group "Corner" "${CORNER_TESTS[@]}"
        run_group "Errata" "${ERRATA_TESTS[@]}"
        ;;
    *)
        run_one "$1"
        ;;
esac
