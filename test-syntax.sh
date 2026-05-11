#!/usr/bin/env bash
# Syntax/parse smoke test for default-profile.sb.
#
# sandbox-exec has no parse-only mode. It parses the profile before applying it,
# so a nested sandbox may still validate syntax even if sandbox_apply itself is
# rejected with "Operation not permitted".

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
PROFILE="${1:-$REPO_DIR/default-profile.sb}"

if [[ ! -f "$PROFILE" ]]; then
    echo "not ok - profile not found: $PROFILE" >&2
    exit 1
fi

RESOLVED_HOME="$(cd "$HOME" && pwd -P)"
RESOLVED_USERS_DIR="$(cd "$RESOLVED_HOME/.." && pwd -P)"
PROJECT_DIR="$(pwd -P)"
RESOLVED_TMPDIR="$(cd "${TMPDIR:-/tmp}" && pwd -P)"

set +e
STDERR="$(sandbox-exec -f "$PROFILE" \
    -D "_USERS_DIR=$RESOLVED_USERS_DIR" \
    -D "_HOME=$RESOLVED_HOME" \
    -D "_PROJECT_DIR=$PROJECT_DIR" \
    -D "_TMPDIR=$RESOLVED_TMPDIR" \
    /usr/bin/true 2>&1)"
STATUS=$?
set -e

if [[ "$STATUS" -eq 0 ]]; then
    echo "ok - profile syntax valid"
    exit 0
fi

if grep -Fq "sandbox_apply: Operation not permitted" <<<"$STDERR"; then
    echo "ok - profile syntax valid; sandbox apply not permitted in this environment"
    exit 0
fi

echo "not ok - profile syntax invalid or sandbox smoke failed" >&2
printf '%s\n' "$STDERR" >&2
exit "$STATUS"
