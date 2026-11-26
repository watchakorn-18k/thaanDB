#!/usr/bin/env bash
# Release notes for a tag, from the commits since the previous tag (all commits for the
# first one), grouped by Conventional Commit type. Used by .github/workflows/release.yml.
# Usage: scripts/release-notes.sh <tag> [repo-url]   (writes Markdown to stdout)
set -euo pipefail

tag="${1:?usage: release-notes.sh <tag> [repo-url]}"
repo="${2:-}"

# The newest tag before this one that is an ancestor of it.
prev=$(git describe --tags --abbrev=0 "$tag^" 2>/dev/null || true)
range="$tag"
[[ -n "$prev" ]] && range="$prev..$tag"

feat="" fix="" perf="" other="" plain=""
re='^([a-z]+)(\([^)]*\))?!?: '
while IFS=$'\x1f' read -r subject hash; do
  [[ -z "$subject" ]] && continue
  line="- ${subject} (${hash})"$'\n'
  if [[ "$subject" =~ $re ]]; then
    case "${BASH_REMATCH[1]}" in
      feat) feat+="$line" ;;
      fix) fix+="$line" ;;
      perf) perf+="$line" ;;
      *) other+="$line" ;;
    esac
  else
    plain+="$line"
  fi
done < <(git log --no-merges --reverse --pretty='%s%x1f%h' "$range")

section() { [[ -n "$2" ]] && printf '### %s\n\n%s\n' "$1" "$2" || true; }

echo "## Changes"
echo
section "Features" "$feat"
section "Fixes" "$fix"
section "Performance" "$perf"
section "Other" "$other"
section "Commits" "$plain"

if [[ -n "$repo" ]]; then
  if [[ -n "$prev" ]]; then
    echo "**Full changelog:** ${repo}/compare/${prev}...${tag}"
  else
    echo "**Full changelog:** ${repo}/commits/${tag}"
  fi
fi
