#!/usr/bin/env bash
#
# Exercise the complete Node HTTP close contract without running the whole workspace. The
# invariant crosses Rust types, the provider FFI, emitted C and a real native socket, so keeping
# the commands together prevents a local change from validating only the layer it happened to
# touch.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

kiln_say "http-close contract: close authority is unforgeable"
kiln_cargo test --locked -p kiln-runtime --doc

kiln_say "http-close contract: graceful close drains the scripted response"
kiln_cargo test --locked -p kiln-runtime \
	scripted_provider_drains_response_before_same_turn_server_close

kiln_say "http-close contract: the provider rejects raw HTTP resource closes"
kiln_cargo test --locked -p kiln-native-number-ffi \
	registered_http_connection_rejects_raw_hostops_close

kiln_say "http-close contract: generated C routes HTTP closes through the realm"
kiln_cargo test --locked -p kiln-backend-c \
	emitted_http_socket_close_is_runtime_owned_and_modeled

kiln_say "http-close contract: native half-close preserves the delayed response"
kiln_cargo test --locked -p kiln-native \
	native_node_http_server_close_waits_for_delayed_response_after_half_close

kiln_say "http-close contract: native abort publishes terminal lifecycle"
kiln_cargo test --locked -p kiln-native \
	native_node_http_socket_destroy_uses_abort_close

kiln_say "http-close contract: passed"
