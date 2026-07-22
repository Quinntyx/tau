#!/usr/bin/env bash
# Exercise the exact built GUI launch path with no pre-existing daemon.
set -Eeuo pipefail

if (( $# != 0 )); then
    printf 'Usage: %s\n' "${0##*/}" >&2
    exit 2
fi

root_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
tau="$root_dir/target/debug/tau"
gui="$root_dir/target/debug/tau-gui"
[[ -x "$tau" ]] || { printf 'missing executable: %s\n' "$tau" >&2; exit 2; }
[[ -x "$gui" ]] || { printf 'missing executable: %s\n' "$gui" >&2; exit 2; }
command -v xvfb-run >/dev/null 2>&1 || { printf 'xvfb-run is required\n' >&2; exit 2; }

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/tau-gui-dogfood.XXXXXX")
launcher_pid=''
cleanup() {
    if [[ -n "$launcher_pid" ]] && kill -0 "$launcher_pid" 2>/dev/null; then
        kill -TERM -- "-$launcher_pid" 2>/dev/null || true
        wait "$launcher_pid" 2>/dev/null || true
    fi
    rm -rf -- "$tmp_dir"
}
trap cleanup EXIT INT TERM

socket="$tmp_dir/tau.sock"
export HOME="$tmp_dir/home"
export XDG_CONFIG_HOME="$tmp_dir/config"
export XDG_DATA_HOME="$tmp_dir/data"
mkdir -p -- "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

cd -- "$root_dir"
setsid xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
    "$tau" --socket "$socket" gui >"$tmp_dir/gui.log" 2>&1 &
launcher_pid=$!

ready=0
for _ in {1..100}; do
    if ! kill -0 "$launcher_pid" 2>/dev/null; then
        printf 'GUI exited during startup\n' >&2
        cat "$tmp_dir/gui.log" >&2 || true
        exit 1
    fi
    if [[ -S "$socket" ]] && "$tau" --socket "$socket" ping 2>/dev/null | grep -qx pong; then
        ready=1
        break
    fi
    sleep 0.1
done
(( ready )) || { printf 'GUI-owned daemon did not become ready\n' >&2; cat "$tmp_dir/gui.log" >&2 || true; exit 1; }

# This delay catches the clone-drop bug that previously killed the daemon
# immediately after the GPUI root and child entities were constructed.
sleep 1
kill -0 "$launcher_pid"
[[ "$("$tau" --socket "$socket" ping)" == pong ]]

kill -TERM -- "-$launcher_pid"
wait "$launcher_pid" 2>/dev/null || true
launcher_pid=''

stopped=0
for _ in {1..50}; do
    if ! "$tau" --socket "$socket" ping >/dev/null 2>&1; then
        stopped=1
        break
    fi
    sleep 0.1
done
(( stopped )) || { printf 'GUI-owned daemon survived GUI shutdown\n' >&2; exit 1; }

printf 'GUI dogfood smoke passed: auto-started daemon lived with the GUI and stopped with it\n'
