#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Jinwoo Park (pmnxis@gmail.com)
#
# SPDX-License-Identifier: MIT OR Apache-2.0

# Build all 26 test binaries.
# Useful before flash session to catch any compile regressions.

set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

echo "Building all test binaries (release)..."
if cargo +nightly build --release --bins 2>&1 | tail -3 | grep -q "Finished"; then
    BIN_COUNT=$(ls src/bin/*.rs | wc -l | tr -d ' ')
    echo -e "${GREEN}All $BIN_COUNT binaries built OK.${NC}"
    exit 0
else
    echo -e "${RED}Build failed. Run with verbose output to see errors:${NC}"
    echo "  cargo +nightly build --release --bins"
    exit 1
fi
