#!/usr/bin/env bash
# Read-only preflight. Making a repository public is a separate maintainer decision.
set -euo pipefail
repo=${GITHUB_REPOSITORY:-sunfall-labs/kiln}
private=$(gh api "repos/$repo" --jq .private)
if [[ $private != false ]]; then
    echo 'release: public publication requires a public repository (private packaging trials are allowed)' >&2
    exit 1
fi
if [[ $(gh api "repos/$repo/private-vulnerability-reporting" --jq .enabled) != true ]]; then
    echo 'release: enable private vulnerability reporting in Settings → Code security before publishing' >&2
    exit 1
fi
