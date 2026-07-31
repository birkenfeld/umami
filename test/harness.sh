#!/usr/bin/env bash
# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.
#
# Dev harness for spinning up a real `umami` instance (and, optionally,
# `umami-gui` under a dedicated Xvfb display) to exercise a change by hand --
# one script invocation per step instead of a fresh set of ad-hoc shell
# commands for building, starting, polling for readiness, driving umami-ctl,
# launching the GUI, and tearing everything down again.
#
# Run `test/harness.sh help` for the command list.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
STATE_DIR="$SCRIPT_DIR/.harness"
DISPLAY_NUM=":99"
XVFB_PIDFILE="$STATE_DIR/xvfb.pid"
XVFB_LOG="$STATE_DIR/xvfb.log"
GUI_PIDFILE="$STATE_DIR/gui.pid"
GUI_LOG="$STATE_DIR/gui.log"

mkdir -p "$STATE_DIR"

usage() {
    cat <<'EOF'
Usage: test/harness.sh <command> [args...]

Instance lifecycle:
  start <conf> [--name NAME] [--release] [--features LIST]
               [--debug] [--raw PATH]
      Start a background `umami` instance. <conf> is a name under test/
      (e.g. "canon.conf") or a full path to a .conf file. NAME defaults to
      <conf>'s basename and is how the other commands refer to this instance.
      Blocks until the instance answers "ping" or the startup fails
      (prints the tail of its log either way).

  stop <name>       Stop an instance and remove its shared-memory segment.
  stop-all          Stop all tracked instances, and the GUI/Xvfb if running.
  status            List tracked instances (and GUI/Xvfb) with their state.
  logs <name> [-n N]
                    Print the last N (default 40) lines of an instance's log.

Talking to a running instance:
  ctl <name> <umami-ctl args...>
      Run umami-ctl against the instance, e.g.:
        test/harness.sh ctl canon state
        test/harness.sh ctl canon start my-run-id

GUI (shared Xvfb display DISPLAY_NUM, started on demand):
  gui <name>              Launch umami-gui against the instance.
  gui-stop                Stop umami-gui and Xvfb.
  screenshot <outfile>    Capture the Xvfb display to a PNG.
  click <x> <y>           Left-click at (x, y) on the display.
  key <keysym>            Send a key/key-combo (xdotool syntax, e.g. "ctrl+q").

Example:
  test/harness.sh start canon.conf
  test/harness.sh ctl canon start my-run-id
  test/harness.sh ctl canon state
  test/harness.sh gui canon
  test/harness.sh screenshot /tmp/shot.png
  test/harness.sh stop canon
EOF
}

# --- instance lifecycle -----------------------------------------------------

cmd_start() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: start <conf> [--name NAME] [--release] [--features LIST] [--debug] [--raw PATH]" >&2
        exit 1
    fi
    local conf_arg="$1"; shift
    local name="" release=0 features="" debug_flag=0 raw=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --release) release=1; shift ;;
            --features) features="$2"; shift 2 ;;
            --debug) debug_flag=1; shift ;;
            --raw) raw="$2"; shift 2 ;;
            *) echo "Unknown option: $1" >&2; exit 1 ;;
        esac
    done

    local conf_path=""
    if [[ -f "$conf_arg" ]]; then
        conf_path="$(cd "$(dirname "$conf_arg")" && pwd)/$(basename "$conf_arg")"
    elif [[ -f "$SCRIPT_DIR/$conf_arg" ]]; then
        conf_path="$SCRIPT_DIR/$conf_arg"
    else
        echo "Config not found: $conf_arg (tried it as a path, and as test/$conf_arg)" >&2
        exit 1
    fi

    [[ -n "$name" ]] || name="$(basename "$conf_path" .conf)"
    local inst_dir="$STATE_DIR/$name"
    if [[ -f "$inst_dir/pid" ]] && kill -0 "$(cat "$inst_dir/pid")" 2>/dev/null; then
        echo "Instance '$name' is already running (pid $(cat "$inst_dir/pid")) -- stop it first." >&2
        exit 1
    fi
    mkdir -p "$inst_dir"

    local profile_args=() features_args=() bin="$ROOT_DIR/target/debug/umami"
    [[ $release -eq 1 ]] && profile_args=(--release) && bin="$ROOT_DIR/target/release/umami"
    [[ -n "$features" ]] && features_args=(--features "$features")
    echo "Building umami..."
    (cd "$ROOT_DIR" && cargo build "${profile_args[@]}" "${features_args[@]}" --bin umami --bin umami-ctl)

    local ipc="harness-$name"
    local args=(--ipc "$ipc")
    [[ $debug_flag -eq 1 ]] && args+=(--debug)
    [[ -n "$raw" ]] && args+=(--raw "$raw")
    args+=("$conf_path")

    # Drop any leftover shared-memory segment from a previous unclean stop
    rm -f "/dev/shm/$ipc"

    echo "$ipc" > "$inst_dir/ipc"
    echo "$conf_path" > "$inst_dir/conf"

    # Config paths (e.g. "data/mesy/00678408.mdat") are resolved relative to
    # the process's cwd, not the config file's location, so we must run from
    # test/ for the checked-in test/data/* files to be found.
    (
        cd "$SCRIPT_DIR"
        setsid "$bin" "${args[@]}" > "$inst_dir/log" 2>&1 < /dev/null &
        echo $! > "$inst_dir/pid"
    )

    echo "Waiting for '$name' (ipc=$ipc) to come up..."
    local tries=0
    until "$ROOT_DIR/target/debug/umami-ctl" --ipc "$ipc" ping >/dev/null 2>&1; do
        tries=$((tries + 1))
        if [[ $tries -ge 50 ]]; then
            echo "Timed out waiting for '$name' to start. Log:" >&2
            tail -n 40 "$inst_dir/log" >&2
            exit 1
        fi
        if ! kill -0 "$(cat "$inst_dir/pid")" 2>/dev/null; then
            echo "'$name' exited during startup. Log:" >&2
            tail -n 40 "$inst_dir/log" >&2
            exit 1
        fi
        sleep 0.2
    done
    echo "Instance '$name' is up (ipc=$ipc, pid=$(cat "$inst_dir/pid"), log=$inst_dir/log)"
}

cmd_stop() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: stop <name>" >&2
        exit 1
    fi
    local name="$1" inst_dir="$STATE_DIR/$1"
    if [[ ! -d "$inst_dir" ]]; then
        echo "No tracked instance named '$name'" >&2
        exit 1
    fi
    local pid ipc
    pid="$(cat "$inst_dir/pid" 2>/dev/null || true)"
    ipc="$(cat "$inst_dir/ipc" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        for _ in $(seq 1 25); do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.2
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    fi
    [[ -n "$ipc" ]] && rm -f "/dev/shm/$ipc"
    rm -rf "$inst_dir"
    echo "Stopped '$name'"
}

cmd_stop_all() {
    if [[ -d "$STATE_DIR" ]]; then
        for d in "$STATE_DIR"/*/; do
            [[ -d "$d" ]] || continue
            cmd_stop "$(basename "$d")" || true
        done
    fi
    cmd_gui_stop
}

cmd_status() {
    local any=0
    if [[ -d "$STATE_DIR" ]]; then
        for d in "$STATE_DIR"/*/; do
            [[ -d "$d" ]] || continue
            any=1
            local n pid ipc conf state
            n="$(basename "$d")"
            pid="$(cat "$d/pid" 2>/dev/null || echo '?')"
            ipc="$(cat "$d/ipc" 2>/dev/null || echo '?')"
            conf="$(cat "$d/conf" 2>/dev/null || echo '?')"
            if kill -0 "$pid" 2>/dev/null; then state="running"; else state="dead"; fi
            echo "$n: $state (pid=$pid ipc=$ipc conf=$conf)"
        done
    fi
    if [[ -f "$XVFB_PIDFILE" ]] && kill -0 "$(cat "$XVFB_PIDFILE")" 2>/dev/null; then
        any=1
        echo "Xvfb: running on $DISPLAY_NUM (pid=$(cat "$XVFB_PIDFILE"))"
    fi
    if [[ -f "$GUI_PIDFILE" ]] && kill -0 "$(cat "$GUI_PIDFILE")" 2>/dev/null; then
        any=1
        echo "umami-gui: running (pid=$(cat "$GUI_PIDFILE"))"
    fi
    [[ $any -eq 1 ]] || echo "Nothing tracked."
}

cmd_logs() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: logs <name> [-n N]" >&2
        exit 1
    fi
    local name="$1"; shift
    local n=40
    if [[ "${1:-}" == "-n" ]]; then
        n="$2"
    fi
    local inst_dir="$STATE_DIR/$name"
    [[ -f "$inst_dir/log" ]] || { echo "No tracked instance named '$name'" >&2; exit 1; }
    tail -n "$n" "$inst_dir/log"
}

# --- talking to an instance --------------------------------------------------

cmd_ctl() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: ctl <name> <umami-ctl args...>" >&2
        exit 1
    fi
    local name="$1"; shift
    local inst_dir="$STATE_DIR/$name"
    [[ -f "$inst_dir/ipc" ]] || { echo "No tracked instance named '$name'" >&2; exit 1; }
    local ipc; ipc="$(cat "$inst_dir/ipc")"
    (cd "$ROOT_DIR" && cargo run -q --bin umami-ctl -- --ipc "$ipc" "$@")
}

# --- GUI (dedicated Xvfb display) --------------------------------------------

ensure_xvfb() {
    if [[ -f "$XVFB_PIDFILE" ]] && kill -0 "$(cat "$XVFB_PIDFILE")" 2>/dev/null; then
        return
    fi
    echo "Starting Xvfb on $DISPLAY_NUM..."
    Xvfb "$DISPLAY_NUM" -screen 0 1280x900x24 > "$XVFB_LOG" 2>&1 &
    echo $! > "$XVFB_PIDFILE"
    sleep 0.5
    if ! kill -0 "$(cat "$XVFB_PIDFILE")" 2>/dev/null; then
        rm -f "$XVFB_PIDFILE"
        echo "Xvfb failed to start on $DISPLAY_NUM (already in use by an untracked process?). Log:" >&2
        tail -n 20 "$XVFB_LOG" >&2
        exit 1
    fi
}

require_xvfb() {
    if ! [[ -f "$XVFB_PIDFILE" ]] || ! kill -0 "$(cat "$XVFB_PIDFILE")" 2>/dev/null; then
        echo "Xvfb is not running (use 'gui' first)" >&2
        exit 1
    fi
}

cmd_gui() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: gui <name>" >&2
        exit 1
    fi
    local name="$1"
    local inst_dir="$STATE_DIR/$name"
    [[ -f "$inst_dir/ipc" ]] || { echo "No tracked instance named '$name'" >&2; exit 1; }
    local ipc; ipc="$(cat "$inst_dir/ipc")"
    ensure_xvfb
    (
        cd "$ROOT_DIR"
        DISPLAY="$DISPLAY_NUM" setsid uv run umami-gui "$ipc" > "$GUI_LOG" 2>&1 < /dev/null &
        echo $! > "$GUI_PIDFILE"
    )
    sleep 1
    echo "umami-gui launched against '$name' on display $DISPLAY_NUM (pid=$(cat "$GUI_PIDFILE"), log=$GUI_LOG)"
}

cmd_gui_stop() {
    if [[ -f "$GUI_PIDFILE" ]]; then
        kill "$(cat "$GUI_PIDFILE")" 2>/dev/null || true
        rm -f "$GUI_PIDFILE"
    fi
    if [[ -f "$XVFB_PIDFILE" ]]; then
        kill "$(cat "$XVFB_PIDFILE")" 2>/dev/null || true
        rm -f "$XVFB_PIDFILE"
    fi
}

cmd_screenshot() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: screenshot <outfile>" >&2
        exit 1
    fi
    require_xvfb
    DISPLAY="$DISPLAY_NUM" import -window root "$1"
    echo "Saved screenshot to $1"
}

cmd_click() {
    if [[ $# -lt 2 ]]; then
        echo "Usage: click <x> <y>" >&2
        exit 1
    fi
    require_xvfb
    DISPLAY="$DISPLAY_NUM" xdotool mousemove --sync "$1" "$2" click 1
}

cmd_key() {
    if [[ $# -lt 1 ]]; then
        echo "Usage: key <keysym>" >&2
        exit 1
    fi
    require_xvfb
    DISPLAY="$DISPLAY_NUM" xdotool key "$1"
}

# --- dispatch -----------------------------------------------------------------

main() {
    local cmd="${1:-}"
    [[ -n "$cmd" ]] || { usage; exit 1; }
    shift
    case "$cmd" in
        start) cmd_start "$@" ;;
        stop) cmd_stop "$@" ;;
        stop-all) cmd_stop_all "$@" ;;
        status) cmd_status "$@" ;;
        logs) cmd_logs "$@" ;;
        ctl) cmd_ctl "$@" ;;
        gui) cmd_gui "$@" ;;
        gui-stop) cmd_gui_stop "$@" ;;
        screenshot) cmd_screenshot "$@" ;;
        click) cmd_click "$@" ;;
        key) cmd_key "$@" ;;
        -h|--help|help) usage ;;
        *) echo "Unknown command: $cmd" >&2; usage; exit 1 ;;
    esac
}

main "$@"
