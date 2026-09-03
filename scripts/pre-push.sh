#!/usr/bin/env bash
set -euo pipefail

# Support --all to force running all checks unconditionally
RUN_ALL=0
if [[ "${1:-}" == "--all" ]]; then
	RUN_ALL=1
fi

CHANGED_FILES=""
if [[ $RUN_ALL -eq 0 ]]; then
	if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
		# Find base commit for comparison against upstream/remote
		BASE=""
		if git rev-parse --verify origin/main >/dev/null 2>&1; then
			BASE="$(git merge-base origin/main HEAD 2>/dev/null || echo "origin/main")"
		elif git rev-parse --verify HEAD~1 >/dev/null 2>&1; then
			BASE="HEAD~1"
		fi

		if [[ -n $BASE ]]; then
			CHANGED_FILES="$(git diff --name-only "$BASE...HEAD" 2>/dev/null || true)"
		fi
	fi
fi

# If we couldn't determine changed files or no base was available, run everything
if [[ $RUN_ALL -eq 0 && -z $CHANGED_FILES ]]; then
	if git rev-parse --verify HEAD~1 >/dev/null 2>&1; then
		CHANGED_FILES="$(git diff --name-only HEAD~1...HEAD 2>/dev/null || true)"
	fi
	if [[ -z $CHANGED_FILES ]]; then
		RUN_ALL=1
	fi
fi

matches_pattern() {
	local pattern="$1"
	if [[ $RUN_ALL -eq 1 ]]; then
		return 0
	fi
	echo "$CHANGED_FILES" | grep -qE "$pattern"
}

# 1. HTTP close contract
echo "=== 1/3: HTTP close contract ==="
if matches_pattern "(crates/kiln-runtime/|crates/kiln-native-number-ffi/src/realm/|crates/kiln-native/src/.*http|scripts/check-http-close-contract\.sh)"; then
	bash scripts/check-http-close-contract.sh
else
	echo "Skipped: no HTTP runtime or close authority changes detected."
fi

# 2. Native number FFI contract
echo "=== 2/3: Native number FFI contract ==="
PROVIDER_PATTERN="(crates/kiln-native-number|crates/kiln-number-core|crates/kiln-temporal|crates/kiln-runtime|crates/kiln-node-path|crates/kiln-artifact-cache|crates/kiln-digest|crates/kiln-host-abi|scripts/check-native-number-ffi\.sh|scripts/package-native-number-provider\.sh)"
if matches_pattern "$PROVIDER_PATTERN"; then
	scripts/check-native-number-ffi.sh
else
	echo "Skipped: no number provider or FFI changes detected."
fi

# 3. Core workspace test suites
echo "=== 3/3: Core workspace test suites ==="
PACKAGES=()

# If global manifest/lockfile or shared core IR changed, test all 5
if matches_pattern "(Cargo\.(toml|lock)|crates/kiln-ir/|crates/kiln-builtin-surface/|crates/kiln-host-abi/)"; then
	PACKAGES=(kiln-cli kiln-daemon kiln-native kiln-backend-c kiln-interpreter)
else
	if matches_pattern "crates/kiln-interpreter/"; then
		PACKAGES+=(kiln-interpreter kiln-native kiln-cli)
	fi
	if matches_pattern "crates/kiln-backend-c/"; then
		PACKAGES+=(kiln-backend-c kiln-native kiln-cli)
	fi
	if matches_pattern "(crates/kiln-native/|crates/kiln-runtime/|crates/kiln-module-graph/)"; then
		PACKAGES+=(kiln-native kiln-cli)
	fi
	if matches_pattern "crates/kiln-daemon/"; then
		PACKAGES+=(kiln-daemon kiln-cli)
	fi
	if matches_pattern "crates/kiln-cli/"; then
		PACKAGES+=(kiln-cli)
	fi
fi

if [[ ${#PACKAGES[@]} -gt 0 ]]; then
	MBX_ARGS=()
	while IFS= read -r pkg; do
		if [[ -n $pkg ]]; then
			MBX_ARGS+=("-p" "$pkg")
		fi
	done < <(printf "%s\n" "${PACKAGES[@]}" | awk '!seen[$0]++')
	echo "Running tests for: ${MBX_ARGS[*]}"
	mbx test "${MBX_ARGS[@]}" --offline
else
	if [[ $RUN_ALL -eq 1 ]]; then
		mbx test -p kiln-cli -p kiln-daemon -p kiln-native -p kiln-backend-c -p kiln-interpreter --offline
	else
		echo "Skipped: no workspace crate changes detected."
	fi
fi

echo "All pre-push checks passed!"
