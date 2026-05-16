#!/usr/bin/env bash
# Syntax/parse smoke test for one or more SBPL profiles.
#
# sandbox-exec has no parse-only mode. It parses the profile before applying it,
# so running /usr/bin/true is only a vehicle for asking sandbox-exec to read the
# passed profile. Operation-not-permitted failures after parsing are considered
# success because they mean the profile was accepted by sandbox-exec.

set -uo pipefail

if [[ "$#" -eq 0 ]]; then
    echo "usage: $0 PROFILE [PROFILE ...]" >&2
    exit 64
fi

STATUS=0

for PROFILE in "$@"; do
    if [[ ! -f "$PROFILE" ]]; then
        echo "not ok - profile not found: $PROFILE" >&2
        STATUS=1
        continue
    fi

    STDERR="$(sandbox-exec -f "$PROFILE" \
        -D "_USERS_DIR=/__seatbelt_syntax__/Users" \
        -D "_HOME=/__seatbelt_syntax__/Users/test" \
        -D "_PROJECT_DIR=/__seatbelt_syntax__/Users/test/project" \
        -D "_TMPDIR=/__seatbelt_syntax__/tmp" \
        /usr/bin/true 2>&1)"
    COMMAND_STATUS=$?

    if [[ "$COMMAND_STATUS" -eq 0 ]]; then
        echo "ok - $PROFILE"
    elif grep -Fq "sandbox_apply: Operation not permitted" <<<"$STDERR"; then
        echo "ok - $PROFILE; parsed, sandbox apply not permitted in this environment"
    elif grep -Fq "execvp() of '/usr/bin/true' failed: Operation not permitted" <<<"$STDERR"; then
        echo "ok - $PROFILE; parsed, profile blocked test command"
    else
        echo "not ok - $PROFILE" >&2
        printf '%s\n' "$STDERR" >&2
        STATUS=1
    fi
done

exit "$STATUS"
