#!/usr/bin/env bash
set -euo pipefail

echo "=== 1/3: HTTP close contract ==="
bash scripts/check-http-close-contract.sh

echo "=== 2/3: Native number FFI contract ==="
scripts/check-native-number-ffi.sh

echo "=== 3/3: Core workspace test suites ==="
mbx test -p kiln-cli -p kiln-daemon -p kiln-native -p kiln-backend-c -p kiln-interpreter --offline

echo "All pre-push checks passed!"
