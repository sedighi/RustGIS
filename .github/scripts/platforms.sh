#!/usr/bin/env bash
# The platforms to test on, as a JSON list of runners. Pushes to main, manual runs and releases
# test on every platform; a pull request on Linux and Windows, plus macOS when it's labelled
# `ci:macos` or `ci:full`. For the label just added (the `CI on label` workflow), only macOS when
# the label asks for it: the pull request's own run already covers Linux and Windows.
#
# From the event: EVENT (github.event_name), ACTION (github.event.action), LABEL (the label
# added) and LABELS (the pull request's labels, as JSON).
set -euo pipefail

# Whether the pull request asks for a platform's label (ci:full asks for every one).
asks() {
  if [ "${ACTION:-}" = labeled ]; then
    [ "${LABEL:-}" = "$1" ] || [ "${LABEL:-}" = ci:full ]
  else
    jq -e --arg l "$1" 'index($l) or index("ci:full")' <<<"${LABELS:-[]}" >/dev/null
  fi
}

os=()
[ "${ACTION:-}" = labeled ] || os+=(ubuntu-latest)
if [ "${EVENT:-}" != pull_request ] || asks ci:macos; then os+=(macos-latest); fi
[ "${ACTION:-}" = labeled ] || os+=(windows-latest)
jq -cn '$ARGS.positional' --args ${os[@]+"${os[@]}"}
