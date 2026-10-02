#!/usr/bin/env bash
# Create docs/adr/NNNN-<slug>.md with the next free number and the usual skeleton.
#
#   scripts/new-adr.sh <slug> "<title>"
#
# Two ADRs were numbered 0059 on 2026-09-13 because two sessions each took "the next number" from
# their own checkout. So the next number here is one above the highest in this checkout *and* on
# origin/main (fetched first when a remote is reachable), and the `adr_numbers_are_unique` crate
# test fails the gate if a collision lands anyway. Commit the new file promptly: an ADR that sits
# uncommitted is invisible to every other session choosing a number.
set -euo pipefail

if [[ $# -ne 2 || ! $1 =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
	printf 'usage: %s <kebab-case-slug> "<title>"\n' "$0" >&2
	exit 2
fi
slug=$1
title=$2
root=$(git rev-parse --show-toplevel)
cd "$root"

git fetch --quiet origin main 2>/dev/null || true

highest() {
	sed -nE 's#^(docs/adr/)?([0-9]{4})-.*#\2#p' | sort -n | tail -1
}
local_max=$(find docs/adr -maxdepth 1 -name '[0-9][0-9][0-9][0-9]-*.md' -exec basename {} \; | highest)
remote_max=$(git ls-tree --name-only origin/main docs/adr/ 2>/dev/null | highest || true)
max=$(printf '%s\n%s\n' "${local_max:-0}" "${remote_max:-0}" | sort -n | tail -1)
number=$(printf '%04d' "$((10#$max + 1))")
path="docs/adr/$number-$slug.md"

cat >"$path" <<ADR
# ADR $number: $title

## Status

Proposed, $(date +%Y-%m-%d).

## Context

## Decision

## Consequences
ADR
printf '%s\n' "$path"
