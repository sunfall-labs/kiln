#!/usr/bin/env bash
set -euo pipefail

# Build the metadata CLI with the repository's shared mbx budget and sanitized host settings.
kiln_provider_contract_init() {
	local root=${1:?repository root required}
	local target_dir=${CARGO_TARGET_DIR:-target}
	local cargo_bin=${KILN_PROVIDER_CARGO:-mbx}
	"$cargo_bin" build --quiet -p kiln-native-provider-contract --bin kiln-native-provider-contract --offline
	KILN_PROVIDER_CONTRACT_BIN="$root/$target_dir/debug/kiln-native-provider-contract"
	[[ -x $KILN_PROVIDER_CONTRACT_BIN ]] || { printf 'provider contract CLI was not built\n' >&2; return 1; }
	KILN_PROVIDER_CONTRACT_JSON=$($KILN_PROVIDER_CONTRACT_BIN contract)
}

kiln_provider_contract_field() {
	local field=$1
	python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field" <<<"$KILN_PROVIDER_CONTRACT_JSON"
}
