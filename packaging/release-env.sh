#!/usr/bin/env bash
# Writes the name, version and release tag of this workflow run as KEY=value
# lines (append the output to $GITHUB_ENV). Inputs, from the workflow:
#   REF_TYPE, REF_NAME   the pushed ref ("tag" + "v0.2.0", or "branch" + "main")
#   INPUT_TAG            a tag typed into "Run workflow" (may be empty)
#   EVENT                the event name (push, pull_request, workflow_dispatch)
#   RUN_NUMBER           this workflow's run number
#   CRASH_TEST           "true" for a deliberately broken beta (rollback test)
#
# A pushed tag or a typed tag is a release. A push to main is an automatic
# beta (see `cargo xtask beta-version`). Any other run (a pull request, a
# hand-started test run) builds with the version an automatic beta of it
# would have, but without a tag, so nothing is published: its installer goes
# over the newest beta, and the update drill can update to it from there.
set -euo pipefail

cargo xtask brand
if [ "${CRASH_TEST:-}" = true ]; then
  cargo xtask beta-version "$RUN_NUMBER"
  echo "BUILD_CRASH_TEST=1"
elif [ "${REF_TYPE:-}" = tag ]; then
  echo "RELEASE_TAG=$REF_NAME"
elif [ -n "${INPUT_TAG:-}" ]; then
  echo "RELEASE_TAG=$INPUT_TAG"
elif [ "${EVENT:-}" = push ] && [ "${REF_NAME:-}" = main ]; then
  cargo xtask beta-version "$RUN_NUMBER"
else
  cargo xtask beta-version "$RUN_NUMBER" | grep -v '^RELEASE_TAG='
fi
