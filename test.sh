#!/usr/bin/env bash
# Minimal behavioral tests for default-profile.sb.

set -euo pipefail

PROFILE="${PWD}/default-profile.sb"
HOME_DIR="$(cd "$HOME" && pwd -P)"
PROJECT_DIR="$(pwd -P)"
TMP_DIR="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
TEST_ID="pi-seatbelt-test-$$"

ensure_fixture_dir() {
    local path="$1"

    if [[ -d "$path" ]]; then
        return 0
    fi

    if mkdir -p "$path" 2>/dev/null; then
        return 0
    fi

    echo "error: required test fixture directory is not available: $path" >&2
    echo "When running test.sh inside the sandbox, create it once outside the sandbox first." >&2
    exit 1
}

ensure_fixture_dir "$HOME_DIR/.cache"
ensure_fixture_dir "$HOME_DIR/.codex/skills"

PROJECT_TEST_DIR="$PROJECT_DIR/$TEST_ID-dir"
mkdir -p "$PROJECT_TEST_DIR"

PROJECT_WRITE="$PROJECT_DIR/$TEST_ID"
CACHE_WRITE="$HOME_DIR/.cache/$TEST_ID"
SKILLS_WRITE="$HOME_DIR/.codex/skills/$TEST_ID"
PI_AGENT_SKILLS_WRITE="$HOME_DIR/.pi/agent/skills/$TEST_ID"
PI_SETTINGS_LOCK_DIR="$HOME_DIR/.pi/agent/settings.json.lock"
PI_AUTH_LOCK_DIR="$HOME_DIR/.pi/agent/auth.json.lock"
TMP_WRITE="$TMP_DIR/$TEST_ID"
HOME_WRITE_DENIED="$HOME_DIR/$TEST_ID"
PI_WRITE_DENIED="$HOME_DIR/.pi/agent/$TEST_ID"
PROJECT_ENV_DENIED="$PROJECT_TEST_DIR/.env"
PROJECT_ENV_DOT_DENIED="$PROJECT_TEST_DIR/.env.$TEST_ID"
PROJECT_ENVRC_DENIED="$PROJECT_TEST_DIR/.envrc"
PROJECT_PEM_DENIED="$PROJECT_TEST_DIR/$TEST_ID.pem"
PROJECT_KEY_DENIED="$PROJECT_TEST_DIR/$TEST_ID.key"

cleanup() {
    rm -f \
        "$PROJECT_WRITE" \
        "$CACHE_WRITE" \
        "$SKILLS_WRITE" \
        "$PI_AGENT_SKILLS_WRITE" \
        "$TMP_WRITE" \
        "$HOME_WRITE_DENIED" \
        "$PI_WRITE_DENIED" \
        "$PROJECT_ENV_DENIED" \
        "$PROJECT_ENV_DOT_DENIED" \
        "$PROJECT_ENVRC_DENIED" \
        "$PROJECT_PEM_DENIED" \
        "$PROJECT_KEY_DENIED"
    rmdir "$PI_SETTINGS_LOCK_DIR" 2>/dev/null || true
    rmdir "$PI_AUTH_LOCK_DIR" 2>/dev/null || true
    rmdir "$PROJECT_TEST_DIR" 2>/dev/null || true
}
trap cleanup EXIT

run_sb() {
    sandbox-exec -f "$PROFILE" \
        -D "_HOME=$HOME_DIR" \
        -D "_PROJECT_DIR=$PROJECT_DIR" \
        -D "_TMPDIR=$TMP_DIR" \
        "$@" >/dev/null 2>&1
}

assert_allowed() {
    local name="$1"
    shift

    if run_sb "$@"; then
        echo "ok - $name"
    else
        echo "not ok - $name" >&2
        exit 1
    fi
}

assert_denied() {
    local name="$1"
    shift

    if run_sb "$@"; then
        echo "not ok - $name" >&2
        exit 1
    else
        echo "ok - $name"
    fi
}

assert_allowed "can stat /Users" /usr/bin/stat -f "%N" /Users
assert_allowed "can stat home root" /usr/bin/stat -f "%N" "$HOME_DIR"
assert_allowed "can read project dir" /bin/ls "$PROJECT_DIR"
assert_allowed "can read ~/.cache" /bin/ls "$HOME_DIR/.cache"
assert_allowed "can read TMPDIR" /bin/ls "$TMP_DIR"
assert_allowed "can read ~/src" /bin/ls "$HOME_DIR/src"
assert_allowed "can read ~/.gitconfig" /bin/cat "$HOME_DIR/.gitconfig"
assert_allowed "can read ~/.gitignore_global" /bin/cat "$HOME_DIR/.gitignore_global"
assert_allowed "can read ~/.local" /bin/ls "$HOME_DIR/.local"
assert_allowed "can read ~/.pi" /bin/ls "$HOME_DIR/.pi"
assert_allowed "can read ~/.nvm" /bin/ls "$HOME_DIR/.nvm"
assert_allowed "can read ~/Library/Keychains" /bin/ls "$HOME_DIR/Library/Keychains"
assert_allowed "can open /dev/null read-write" /bin/sh -c 'exec 3<>/dev/null'

assert_allowed "can write project dir" /usr/bin/touch "$PROJECT_WRITE"
assert_allowed "can write ~/.cache" /usr/bin/touch "$CACHE_WRITE"
assert_allowed "can write ~/.codex/skills" /usr/bin/touch "$SKILLS_WRITE"
assert_allowed "can write ~/.pi/agent/skills" /usr/bin/touch "$PI_AGENT_SKILLS_WRITE"
assert_allowed "can create Pi settings lock dir" /bin/mkdir "$PI_SETTINGS_LOCK_DIR"
assert_allowed "can create Pi auth lock dir" /bin/mkdir "$PI_AUTH_LOCK_DIR"
assert_allowed "can write TMPDIR" /usr/bin/touch "$TMP_WRITE"

assert_denied "denies write of home root" /usr/bin/touch "$HOME_WRITE_DENIED"
assert_denied "denies write of ~/.pi/agent" /usr/bin/touch "$PI_WRITE_DENIED"
assert_denied "denies write of project .env" /usr/bin/touch "$PROJECT_ENV_DENIED"
assert_denied "denies write of project .env.*" /usr/bin/touch "$PROJECT_ENV_DOT_DENIED"
assert_denied "denies write of project .envrc" /usr/bin/touch "$PROJECT_ENVRC_DENIED"
assert_denied "denies write of project *.pem" /usr/bin/touch "$PROJECT_PEM_DENIED"
assert_denied "denies write of project *.key" /usr/bin/touch "$PROJECT_KEY_DENIED"

assert_denied "denies listing /Users" /bin/ls /Users
assert_denied "denies listing home root" /bin/ls "$HOME_DIR"
assert_denied "denies read of ~/Library" /bin/ls "$HOME_DIR/Library"
assert_denied "denies read of ~/.zshrc" /bin/cat "$HOME_DIR/.zshrc"
assert_denied "denies read of ~/.zshenv" /bin/cat "$HOME_DIR/.zshenv"

assert_wrapper_help() {
    local output

    if ! output="$("$PROJECT_DIR/sb" --help)"; then
        echo "not ok - wrapper accepts --help" >&2
        exit 1
    fi

    if grep -Fqx "Usage: sb [options] <command> [args...]" <<<"$output" \
        && grep -Fqx "  -h, --help         Show this help message" <<<"$output"; then
        echo "ok - wrapper accepts --help"
    else
        echo "not ok - wrapper accepts --help" >&2
        exit 1
    fi

    if "$PROJECT_DIR/sb" -h >/dev/null; then
        echo "ok - wrapper accepts -h"
    else
        echo "not ok - wrapper accepts -h" >&2
        exit 1
    fi
}

assert_wrapper_env() {
    local output

    if ! output="$(ALLOWED_ONE="one" \
        ALLOWED_TWO="two" \
        SECRET_SHOULD_NOT_PASS="hidden" \
        TEST_BASE_URL="https://example.invalid" \
        "$PROJECT_DIR/sb" \
        --profile "$PROFILE" \
        --allow-env=ALLOWED_ONE \
        --allow-env ALLOWED_TWO \
        /usr/bin/env 2>/dev/null)"; then
        echo "not ok - wrapper passes through allowed env" >&2
        exit 1
    fi

    if grep -Fqx "ALLOWED_ONE=one" <<<"$output" \
        && grep -Fqx "ALLOWED_TWO=two" <<<"$output" \
        && ! grep -q "^SECRET_SHOULD_NOT_PASS=" <<<"$output" \
        && ! grep -q "^TEST_BASE_URL=" <<<"$output"; then
        echo "ok - wrapper passes through allowed env"
    else
        echo "not ok - wrapper passes through allowed env" >&2
        exit 1
    fi
}

assert_wrapper_missing_env_denied() {
    if (unset MISSING_ALLOWED_ENV; "$PROJECT_DIR/sb" --profile "$PROFILE" --allow-env=MISSING_ALLOWED_ENV /usr/bin/true >/dev/null 2>&1); then
        echo "not ok - wrapper rejects missing allowed env" >&2
        exit 1
    else
        echo "ok - wrapper rejects missing allowed env"
    fi
}

assert_wrapper_help
assert_wrapper_env
assert_wrapper_missing_env_denied
