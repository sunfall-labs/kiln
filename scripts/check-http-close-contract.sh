#!/usr/bin/env bash
#
# Exercise the complete Node HTTP close contract without running the whole workspace. The
# invariant crosses runtime ownership, the provider FFI, emitted C and a real native socket.
# Keeping the commands together prevents a local change from validating only the layer it happened
# to touch.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

HTTP_CLOSE_LOG_DIR=${KILN_HTTP_CLOSE_LOG_DIR:-target/http-close-contract/run.$(date -u +%Y%m%dT%H%M%SZ).$$}
mkdir -p "$HTTP_CLOSE_LOG_DIR"

kiln_http_close_test() {
	local step="$1"
	local log_name="$2"
	shift 2
	local log_path="$HTTP_CLOSE_LOG_DIR/$log_name.log"

	kiln_say "http-close contract: $step"
	if ! kiln_cargo "$@" 2>&1 | tee "$log_path"; then
		return 1
	fi
	if ! awk '
		$1 == "test" && $2 == "result:" && $3 == "ok." &&
		$4 ~ /^[0-9]+$/ && $5 == "passed;" { passed += $4 }
		END { exit !(passed > 0) }
	' "$log_path"; then
		printf 'http-close contract: %s ran no passing tests; full output: %s\n' \
			"$step" "$log_path" >&2
		return 1
	fi
}

kiln_http_close_test \
	"unattached resources cannot be closed or keep the realm alive" \
	unattached-resource \
	test --locked -p kiln-runtime \
	unattached_http_resource_cannot_be_closed_or_keep_the_realm_alive

kiln_http_close_test \
	"graceful close drains the scripted response" \
	scripted-drain \
	test --locked -p kiln-runtime \
	scripted_provider_drains_response_before_same_turn_server_close

kiln_http_close_test \
	"the provider rejects raw HTTP resource closes" \
	provider-raw-close \
	test --locked -p kiln-native-provider-ffi \
	registered_http_connection_rejects_raw_hostops_close

kiln_http_close_test \
	"generated C routes HTTP closes through the realm" \
	emitted-c-routing \
	test --locked -p kiln-backend-c \
	emitted_http_socket_close_is_runtime_owned_and_modeled

kiln_http_close_test \
	"native half-close preserves the delayed response" \
	native-half-close \
	test --locked -p kiln-native \
	native_node_http_server_close_waits_for_delayed_response_after_half_close

kiln_http_close_test \
	"native abort publishes terminal lifecycle" \
	native-abort \
	test --locked -p kiln-native \
	native_node_http_socket_destroy_uses_abort_close

kiln_say "http-close contract: passed (full command logs: $HTTP_CLOSE_LOG_DIR)"
