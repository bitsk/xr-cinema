#!/usr/bin/env bash
# Run under Linux with Monado service, OpenXR loader, Mesa lavapipe, Python 3
# and a working GStreamer installation. ffmpeg is used only without a clip.
set -euo pipefail
if (( $# < 1 || $# > 2 )); then
    echo "Usage: bash xr-smoke.sh /path/to/xr-cinema [short-video-file]" >&2
    exit 2
fi
player=$(realpath "$1")
clip=${2:-}
test -x "$player"
if [[ -z "${DISPLAY:-}" ]] && command -v xvfb-run >/dev/null; then
    # Keep the wrapper below PID 1 so Xvfb can signal its readiness in Docker.
    xvfb-run -a -s '-screen 0 640x480x24 -nolisten tcp' bash "$0" "$@"
    exit $?
fi

work=$(mktemp -d)
export XDG_RUNTIME_DIR="$work/runtime" XDG_CONFIG_HOME="$work/config"
export XDG_DATA_HOME="$work/data" XDG_CACHE_HOME="$work/cache"
export SIMULATED_ENABLE=1 XRT_COMPOSITOR_NULL=1 XRT_COMPOSITOR_SCALE_PERCENTAGE=30
export XR_CINEMA_AUDIO_SINK=fakesink
server= app=
cleanup() {
    status=$?
    trap - EXIT
    if [[ -n "$app" ]]; then kill "$app" 2>/dev/null || :; wait "$app" 2>/dev/null || :; fi
    if [[ -n "$server" ]]; then kill "$server" 2>/dev/null || :; wait "$server" 2>/dev/null || :; fi
    if (( status != 0 )); then
        cat "$work/"*.log >&2 2>/dev/null || :
    fi
    rm -rf -- "$work"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

export XR_RUNTIME_JSON=${MONADO_MANIFEST:-$(find /usr/share/openxr/1 -iname '*monado*.json' -print -quit)}
export VK_DRIVER_FILES=${LAVAPIPE_ICD:-$(find /usr/share/vulkan/icd.d -iname 'lvp_icd*.json' -print -quit)}
[[ -r "$XR_RUNTIME_JSON" && -r "$VK_DRIVER_FILES" ]] || {
    echo "Missing Monado or lavapipe manifest. Set MONADO_MANIFEST and LAVAPIPE_ICD." >&2
    exit 2
}
if [[ -z "$clip" ]]; then
    clip="$work/pattern.webm"
    ffmpeg -hide_banner -loglevel error -nostdin -f lavfi -i testsrc2=size=128x64:rate=12 \
        -f lavfi -i sine=frequency=400 -t 12 -c:v libvpx -c:a libvorbis "$clip"
fi
clip=$(realpath "$clip")
test -r "$clip"
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkfifo "$work/stdin"
exec 3<>"$work/stdin"
monado-service <&3 >"$work/monado.log" 2>&1 &
server=$!
for ((n=0; n<100; n++)); do
    kill -0 "$server"
    if [[ -n "$(find "$XDG_RUNTIME_DIR" -type s -print -quit)" ]]; then break; fi
    sleep 0.2
done
[[ -n "$(find "$XDG_RUNTIME_DIR" -type s -print -quit)" ]] || { echo 'Monado IPC socket timed out' >&2; exit 1; }
"$player" --library "$work" >"$work/cinema.log" 2>&1 &
app=$!
python3 - "$clip" "$app" <<'PY'
import itertools
import json
import os
from pathlib import Path
import socket
import sys
import time

sockpath = Path(os.environ["XDG_RUNTIME_DIR"]) / "xr-cinema/control.sock"
ids = itertools.count(1)
clip, pid = sys.argv[1], int(sys.argv[2])

def command(op, **args):
    identifier = next(ids)
    data = {"version": 1, "id": identifier, "command": {"op": op, **args}}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(7)
        client.connect(str(sockpath))
        client.sendall(json.dumps(data).encode() + b"\n")
        with client.makefile("rb") as reply:
            response = json.loads(reply.readline(16385))
    if response.get("id") != identifier or not response.get("ok"):
        raise RuntimeError(response)
    return response["state"]

def wait_for(label, predicate):
    deadline = time.monotonic() + 90
    state = None
    while time.monotonic() < deadline:
        os.kill(pid, 0)
        try:
            state = command("status")
        except (FileNotFoundError, ConnectionRefusedError, TimeoutError):
            time.sleep(0.15)
            continue
        except RuntimeError as error:
            # The control socket appears before the Vulkan pipeline is built.
            # CPU rendering on emulated targets can outlast a single IPC call.
            if state is None and "timed out" in str(error).lower():
                time.sleep(0.15)
                continue
            raise
        if state.get("error"):
            raise RuntimeError(state["error"])
        if predicate(state):
            return state
        time.sleep(0.1)
    raise RuntimeError(f"Timed out waiting for {label}: {state}")

wait_for("first stereo frames", lambda s: s["submitted_frames"] >= 2)
command("open", path=clip)
command("play")
state = wait_for("decoded video and playback clock", lambda s: s["path"] == clip and s["position"] > 0.2 and s["decoded_frames"] > 0 and s["video_size"] and s["duration"])
duration = state["duration"]
target = duration * 0.4
start = command("pause")["submitted_frames"]
wait_for("pause", lambda s: not s["playing"] and s["submitted_frames"] >= start + 2)
start = command("seek", seconds=target)["submitted_frames"]
wait_for("paused seek", lambda s: not s["playing"] and abs(s["position"] - target) < 0.4 and s["submitted_frames"] >= start + 2)
for projection, stereo in [("flat", "mono"), ("flat", "sbs"), ("equirect180", "top_bottom"), ("equirect360", "sbs")]:
    command("set_projection", projection=projection)
    start = command("set_stereo", stereo=stereo)["submitted_frames"]
    state = wait_for(projection, lambda s: s["projection"] == projection and s["stereo"] == stereo and s["submitted_frames"] >= start + 2)
    print(f"{projection} {stereo}: frame {state['submitted_frames']}", flush=True)

command("swap_eyes", enabled=True)
command("set_scene", scene="void")
command("show_panel", visible=False)
command("volume", level=0.35)
start = command("recenter")["submitted_frames"]
state = wait_for("scene and controller settings", lambda s: s["scene"] == "void" and s["swap_eyes"] and not s["panel_visible"] and abs(s["volume"] - 0.35) < 0.01 and s["submitted_frames"] >= start + 2)
command("seek", seconds=max(0, duration - 1))
command("play")
wait_for("end of stream", lambda s: s["ended"])
command("play")
state = wait_for("replay", lambda s: s["playing"] and not s["ended"] and s["position"] < duration - 0.2)
command("quit")
print(f"Simulated stereo submission and control checks passed ({state['submitted_frames']} frames).")
PY
for ((n=0; n<50; n++)); do
    if ! kill -0 "$app" 2>/dev/null; then break; fi
    sleep 0.2
done
if kill -0 "$app" 2>/dev/null; then echo 'Cinema did not exit after quit' >&2; exit 1; fi
wait "$app"
app=
