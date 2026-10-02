#!/usr/bin/env bash
# Refuse a change that deletes an issue filing under docs/issues/ without moving it.
#
#   scripts/check-issue-deletions.sh --staged      what `git commit` is about to record
#   scripts/check-issue-deletions.sh A..B          what a range of commits did (pre-push)
#
# On 2026-10-02 a commit about the GUI run loop also deleted all 634 tracked filings, because they
# had been cleared from disk and `git add -A` recorded that. Nothing noticed for hours. A closed
# issue keeps its file with `Status: closed`; deleting one is a decision, so it has to be said:
# `KILN_ALLOW_ISSUE_DELETION=1` for that one command. Moves within docs/issues/ are always fine.
set -euo pipefail

if [[ -n ${KILN_ALLOW_ISSUE_DELETION:-} ]]; then
	exit 0
fi

case "${1:-}" in
--staged) diff_args=(--cached) ;;
*..*) diff_args=("$1") ;;
*)
	printf 'usage: %s --staged | A..B\n' "$0" >&2
	exit 2
	;;
esac

# -M pairs a delete with its add, so a rename shows as one R line naming both paths.
removed=$(git diff --name-status -M "${diff_args[@]}" -- docs/issues |
	awk -F'\t' '
		$1 == "D" { print $2 }
		$1 ~ /^R/ && $3 !~ /^docs\/issues\// { print $2 }
	')

if [[ -z $removed ]]; then
	exit 0
fi

count=$(printf '%s\n' "$removed" | wc -l | tr -d ' ')
{
	printf 'check-issue-deletions: this change deletes %s issue filing(s) under docs/issues/:\n' "$count"
	printf '%s\n' "$removed" | head -20 | sed 's/^/  /'
	if ((count > 20)); then
		printf '  ... and %s more\n' "$((count - 20))"
	fi
	# shellcheck disable=SC2016 # the backticks are Markdown, not command substitution
	printf '\nA closed issue keeps its file with `Status: closed`. If the files vanished from disk,\n'
	# shellcheck disable=SC2016
	printf 'restore them with `git checkout HEAD -- docs/issues` and commit again. If deleting them\n'
	printf 'is the decision, say so for this one command: KILN_ALLOW_ISSUE_DELETION=1 git ...\n'
} >&2
exit 1
