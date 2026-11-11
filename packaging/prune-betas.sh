#!/usr/bin/env bash
# Removes old automatic betas from the releases page, with their tags.
#
#   prune-betas.sh <owner/repo> [--dry-run]
#
# Keeps the newest KEEP (default 10) releases tagged vX.Y.Z-beta.N. Stable
# releases, drafts and anything tagged differently are never touched.
# Installed copies only ever want the newest beta, a rollback uses the copy
# already on the machine, and versions before 0.1.1-beta.103 only look at
# the first 20 releases GitHub lists, in an order that is not newest first.
#
# Needs the GitHub CLI and a token that may write releases (GH_TOKEN).
set -euo pipefail

repo=${1:?"usage: prune-betas.sh <owner/repo> [--dry-run]"}
dry_run=${2:-}
keep=${KEEP:-10}

# Tags of the betas to remove, read from a list of tags on standard input:
# everything after the newest $1, by version number.
old_betas() {
  grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$' | sort --version-sort --reverse | tail -n "+$(($1 + 1))"
}

if [ "$repo" = --self-test ]; then
  got=$(printf '%s\n' v0.1.0 v0.1.1-beta.99 v0.1.1-beta.101 v0.1.1-beta.8 v0.1.2-beta.3 nightly v0.1.1-beta.36 | old_betas 2 | tr '\n' ' ')
  want="v0.1.1-beta.99 v0.1.1-beta.36 v0.1.1-beta.8 "
  [ "$got" = "$want" ] || { echo "old_betas gave '$got', expected '$want'" >&2; exit 1; }
  echo "prune-betas: self-test passed"
  exit 0
fi

tags=$(gh api --paginate "repos/$repo/releases?per_page=100" \
  --jq '.[] | select(.prerelease and (.draft | not)) | .tag_name')
old=$(printf '%s\n' "$tags" | old_betas "$keep" || true)
if [ -z "$old" ]; then
  echo "No old betas to remove."
  exit 0
fi
for tag in $old; do
  if [ "$dry_run" = --dry-run ]; then
    echo "Would remove $tag"
  else
    gh release delete "$tag" --repo "$repo" --cleanup-tag --yes
    echo "Removed $tag"
  fi
done
