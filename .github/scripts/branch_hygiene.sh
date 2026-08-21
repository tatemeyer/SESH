#!/usr/bin/env bash
# Detects the stale-base / stranded-merge failure described in issue #94, which has now happened
# four times across three repos (Model-Experiments #26/#86/#88, SESH #39-#41, Parallax #44).
#
# Repo-agnostic by construction -- it resolves the default branch at runtime and takes the repo
# from GITHUB_REPOSITORY (or $1) -- so parallax and SESH can copy this file and its workflow
# verbatim. Nothing here is specific to this repo's projects, languages, or CI.
#
# Requires: gh (authenticated), git, jq, and a full-history checkout (fetch-depth: 0).
#
# Exit 0 = clean (or only allowlisted findings), 1 = at least one unexplained finding.

set -uo pipefail

REPO="${1:-${GITHUB_REPOSITORY:-}}"
if [ -z "$REPO" ]; then
    echo "usage: branch_hygiene.sh <owner/repo>   (or set GITHUB_REPOSITORY)" >&2
    exit 2
fi

ALLOWLIST="${ALLOWLIST_PATH:-.github/stale-base-allowlist.txt}"
# How far back to look for stranded merges. The open-PR check below is never windowed -- an open
# PR is actionable no matter how old -- but re-reporting long-settled history forever is how a
# guard becomes noise that gets ignored, which is the failure mode this guard exists to prevent.
LOOKBACK_DAYS="${LOOKBACK_DAYS:-30}"

DEFAULT_BRANCH=$(gh api "repos/$REPO" --jq .default_branch)
echo "repo=$REPO default_branch=$DEFAULT_BRANCH lookback=${LOOKBACK_DAYS}d"

git fetch --quiet origin "$DEFAULT_BRANCH" || true
DEFAULT_REF="origin/$DEFAULT_BRANCH"
git rev-parse --verify --quiet "$DEFAULT_REF" >/dev/null || DEFAULT_REF="$DEFAULT_BRANCH"

is_allowlisted() {
    [ -f "$ALLOWLIST" ] || return 1
    grep -qE "^[[:space:]]*$1([[:space:]]|$)" "$ALLOWLIST"
}

findings=0
summary() { if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo -e "$1" >>"$GITHUB_STEP_SUMMARY"; fi; }

summary "## Branch hygiene ($REPO)\n"

# ---------------------------------------------------------------------------
# Check A -- open PRs targeting something other than the default branch.
#
# This is the high-confidence check: zero false positives, and it fires *before* the stranding
# happens rather than after, which is the only point at which the fix is still cheap. Every
# incident in issue #94 was visible here first.
# ---------------------------------------------------------------------------
echo
echo "== A: open PRs not targeting $DEFAULT_BRANCH =="
summary "### A. Open PRs not targeting \`$DEFAULT_BRANCH\`\n"
open_bad=$(gh pr list --repo "$REPO" --state open --limit 200 \
    --json number,title,baseRefName,headRefName,isDraft \
    --jq ".[] | select(.baseRefName != \"$DEFAULT_BRANCH\") | \"\(.number)\t\(.baseRefName)\t\(.headRefName)\t\(.title)\"")

if [ -z "$open_bad" ]; then
    echo "  none"
    summary "None. :white_check_mark:\n"
else
    while IFS=$'\t' read -r num base head title; do
        [ -z "$num" ] && continue
        if is_allowlisted "#$num"; then
            echo "  #$num -> $base (allowlisted)"
            summary "- #$num → \`$base\` — allowlisted\n"
            continue
        fi
        echo "  #$num  base=$base  head=$head  -- $title"
        summary "- **#$num** targets \`$base\` (not \`$DEFAULT_BRANCH\`) — head \`$head\`\n"
        findings=$((findings + 1))
    done <<<"$open_bad"
fi

# ---------------------------------------------------------------------------
# Check B -- merged PRs whose merge commit never reached the default branch.
#
# Lower confidence than A, deliberately. When a stacked child merges into its parent and the
# parent is then *squash*-merged, the child's merge commit is legitimately not an ancestor of the
# default branch even though its content shipped -- Model-Experiments #26/#86/#88 are all exactly
# that, verified by hand. So a hit here means "prove the content landed", not "work was lost".
# Confirmed-benign cases go in the allowlist so this converges to silence instead of crying wolf.
# ---------------------------------------------------------------------------
echo
echo "== B: merged PRs whose merge commit is not an ancestor of $DEFAULT_BRANCH (last ${LOOKBACK_DAYS}d) =="
summary "### B. Merged PRs not on \`$DEFAULT_BRANCH\` (last ${LOOKBACK_DAYS} days)\n"
cutoff=$(date -u -d "${LOOKBACK_DAYS} days ago" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
    || date -u -v-"${LOOKBACK_DAYS}"d +%Y-%m-%dT%H:%M:%SZ)

merged=$(gh pr list --repo "$REPO" --state merged --limit 200 \
    --json number,title,mergeCommit,baseRefName,mergedAt \
    --jq ".[] | select(.mergeCommit != null) | select(.mergedAt > \"$cutoff\") | \"\(.number)\t\(.mergeCommit.oid)\t\(.baseRefName)\t\(.title)\"")

stranded=0
if [ -n "$merged" ]; then
    while IFS=$'\t' read -r num oid base title; do
        [ -z "$num" ] && continue
        if git merge-base --is-ancestor "$oid" "$DEFAULT_REF" 2>/dev/null; then
            continue
        fi
        stranded=$((stranded + 1))
        if is_allowlisted "#$num"; then
            echo "  #$num  merge=$oid  (allowlisted)"
            summary "- #$num — not an ancestor, allowlisted as verified-benign\n"
            continue
        fi
        echo "  #$num  base=$base  merge=$oid  NOT on $DEFAULT_BRANCH  -- $title"
        summary "- **#$num** merged into \`$base\`; merge commit \`${oid:0:12}\` is not on \`$DEFAULT_BRANCH\` — **verify the content actually landed**\n"
        findings=$((findings + 1))
    done <<<"$merged"
fi
[ "$stranded" -eq 0 ] && { echo "  none"; summary "None. :white_check_mark:\n"; }

echo
if [ "$findings" -gt 0 ]; then
    echo "FAIL: $findings unexplained finding(s). See issue #94 for background."
    summary "\n**$findings unexplained finding(s).** Background: issue #94.\n"
    summary "\nIf a Check B hit is a benign squash-stack artifact, confirm the content is on \`$DEFAULT_BRANCH\` and add the PR number to \`$ALLOWLIST\` with a reason.\n"
    exit 1
fi
echo "OK: no unexplained findings."
summary "\nNo unexplained findings. :white_check_mark:\n"
