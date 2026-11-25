"""Exercise installed demo IDs, layout selection and actual GStreamer decoding."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time


def main(binary, catalog):
    catalog = Path(catalog).resolve()
    items = json.loads(catalog.read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory(prefix="cinema-gallery-") as runtime:
        env = dict(os.environ, XDG_RUNTIME_DIR=runtime, XR_CINEMA_AUDIO_SINK="fakesink")
        env.pop("XR_OPENXR_RUNTIME_DIR", None)
        path = Path(runtime) / "xr-cinema/control.sock"
        player = subprocess.Popen([binary, "--headless", "--library", runtime], env=env)

        def command(op, **args):
            request = {"version": 1, "id": "gallery-test", "command": {"op": op, **args}}
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(8)
                connection.connect(str(path))
                connection.sendall(json.dumps(request).encode() + b"\n")
                with connection.makefile("rb") as stream:
                    reply = json.loads(stream.readline(16385))
            if not reply["ok"]:
                raise RuntimeError(reply["error"])
            return reply["state"]

        def wait(predicate):
            deadline = time.monotonic() + 35
            while time.monotonic() < deadline and player.poll() is None:
                if path.is_socket():
                    state = command("status")
                    if state["error"]:
                        raise RuntimeError(state["error"])
                    if predicate(state):
                        return state
                time.sleep(0.05)
            raise RuntimeError(f"timed out waiting for {predicate}")

        try:
            wait(lambda s: s["ready"])
            for item in items:
                opened = command("open_demo", id=item["id"])
                assert opened["demo_id"] == item["id"]
                assert opened["projection"] == item["projection"] and opened["stereo"] == item["stereo"]
                assert opened["squeezed"] == item.get("squeezed", False) and not opened["swap_eyes"]
                state = wait(lambda s: s["decoded_frames"] >= 2 and s["duration"] and s["playing"])
                assert Path(state["path"]) == catalog.parent.parent / item["file"]
                print(f"{item['id']:14s} {state['video_size']} {state['duration']:.2f}s "
                      f"{state['projection']}/{state['stereo']} — {item['title']}", flush=True)
                command("pause")
                before = command("status")["decoded_frames"]
                command("seek", seconds=5)
                wait(lambda s: not s["playing"] and s["decoded_frames"] > before
                     and abs(s["position"] - 5) < 0.2)
            assert command("next")["demo_id"] == items[0]["id"]
            assert command("previous")["demo_id"] == items[-1]["id"]
            command("quit")
            assert player.wait(timeout=8) == 0
        finally:
            if player.poll() is None:
                player.terminate()
                player.wait(timeout=8)


if __name__ == "__main__":
    main(*sys.argv[1:])
