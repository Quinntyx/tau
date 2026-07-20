#!/usr/bin/env bash
# Start a local tau daemon and exercise its credential-free RPCs.
#
# Set TAU_BIN to an already-built tau executable to avoid a Cargo invocation;
# otherwise this uses Cargo in offline mode so the smoke test never downloads
# dependencies or contacts a model provider.
set -Eeuo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    printf 'Usage: %s [--help]\n\n' "${0##*/}"
    printf 'Launch an isolated local tau daemon and verify ping and health.\n'
    printf 'Set TAU_BIN to use an existing executable instead of Cargo.\n'
    exit 0
fi
if (( $# != 0 )); then
    printf 'unexpected argument: %s\n' "$1" >&2
    printf 'try --help\n' >&2
    exit 2
fi

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/tau-launch-smoke.XXXXXX")
daemon_pid=''
tool_home=${HOME:-}

cleanup() {
    if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
        kill "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    rm -rf -- "$tmp_dir"
}
trap cleanup EXIT INT TERM

socket="$tmp_dir/tau.sock"
# Keep the installed offline toolchain and dependency cache available while
# isolating tau's user-facing home/config/data directories below.
export RUSTUP_HOME="${RUSTUP_HOME:-$tool_home/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$tool_home/.cargo}"
export HOME="$tmp_dir/home"
export XDG_CONFIG_HOME="$tmp_dir/config"
export XDG_DATA_HOME="$tmp_dir/data"
mkdir -p -- "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

if [[ -n "${TAU_BIN:-}" ]]; then
    [[ -x "$TAU_BIN" ]] || {
        printf 'TAU_BIN is not executable: %s\n' "$TAU_BIN" >&2
        exit 2
    }
    tau=("$TAU_BIN")
else
    tau=(cargo run --offline --quiet --manifest-path "$root_dir/Cargo.toml" -p tau-cli --)
fi

# Keep both daemon and client invocations tied to this checkout; no default
# socket or user configuration can leak into the acceptance run.
cd -- "$root_dir"
"${tau[@]}" --socket "$socket" serve >"$tmp_dir/daemon.log" 2>&1 &
daemon_pid=$!

ready=0
# A clean CI checkout may need to link tau-cli before the socket exists.
for _ in {1..600}; do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        printf 'tau daemon exited before becoming ready\n' >&2
        cat "$tmp_dir/daemon.log" >&2 || true
        exit 1
    fi
    if [[ -S "$socket" ]] && "${tau[@]}" --socket "$socket" ping 2>/dev/null | grep -qx 'pong'; then
        ready=1
        break
    fi
    sleep 0.1
done

if (( ! ready )); then
    printf 'tau daemon did not become ready\n' >&2
    cat "$tmp_dir/daemon.log" >&2 || true
    exit 1
fi
ping=$("${tau[@]}" --socket "$socket" ping)
health=$("${tau[@]}" --socket "$socket" health)
[[ "$ping" == pong ]] || { printf 'unexpected ping response: %s\n' "$ping" >&2; exit 1; }
[[ "$health" == version=* ]] || { printf 'unexpected health response: %s\n' "$health" >&2; exit 1; }
printf 'launch smoke passed: %s; %s\n' "$ping" "$health"
