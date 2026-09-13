#!/usr/bin/env bash
set -euo pipefail

# Build the metadata CLI with the repository's shared mbx budget and sanitized host settings.
kiln_provider_contract_init() {
	local root=${1:?repository root required}
	local target_dir=${CARGO_TARGET_DIR:-target}
	local cargo_bin=${KILN_PROVIDER_CARGO:-mbx}
	[[ $target_dir = /* ]] || target_dir="$root/$target_dir"
	local host_target
	host_target=$(env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS -u CARGO_BUILD_TARGET \
		-u CARGO_BUILD_RUSTC -u RUSTC_WRAPPER rustc -vV | sed -n 's/^host: //p')
	env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS -u CARGO_BUILD_TARGET \
		-u CARGO_BUILD_RUSTC -u RUSTC_WRAPPER -u RUSTC_WORKSPACE_WRAPPER \
		-u CARGO_BUILD_RUSTC_WRAPPER -u CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER \
		CARGO_TARGET_DIR="$target_dir" "$cargo_bin" build --quiet -p kiln-native-provider-contract \
		--bin kiln-native-provider-contract --target "$host_target" --offline
	KILN_PROVIDER_CONTRACT_BIN="$target_dir/$host_target/debug/kiln-native-provider-contract"
	[[ -x $KILN_PROVIDER_CONTRACT_BIN ]] || { printf 'provider contract CLI was not built\n' >&2; return 1; }
	KILN_PROVIDER_CONTRACT_JSON=$("$KILN_PROVIDER_CONTRACT_BIN" contract)
}

kiln_provider_contract_field() {
	local field=$1
	python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$field" <<<"$KILN_PROVIDER_CONTRACT_JSON"
}
