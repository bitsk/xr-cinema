#!/usr/bin/env python3
"""Bring the cinema into view when launched from the spatial desktop."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from runtime_control import ACTIVE, PRIMARY, Monado, WiVRnControl, runtime_details, runtime_directory_for

NAME = "openKylin XR Cinema"
CONTROL_ERRORS = (OSError, ValueError, RuntimeError, AttributeError, KeyError, subprocess.TimeoutExpired)


def cinema(clients):
    return next((c for c in clients if c.name == NAME and c.flags & ACTIVE), None)


def restore(controller, previous, player):
    if previous is None or player is None or previous.id == player.id:
        return
    clients = controller.clients()
    current = next((c for c in clients if c.flags & PRIMARY), None)
    if current and (current.id, current.name) not in {
        (previous.id, previous.name), (player.id, player.name)
    }:
        return
    prior = next((c for c in clients if c.id == previous.id and c.name == previous.name and c.flags & ACTIVE), None)
    if prior:
        controller.show(prior)


def launch(arguments):
    # Resolve relative to the installed launcher even with a non-default prefix.
    binary = Path(__file__).resolve().parents[2] / "bin/xr-cinema"
    command = [str(binary) if binary.is_file() else "xr-cinema", *arguments]
    environment = dict(os.environ)
    try:
        backend, library = runtime_details(environment)
    except CONTROL_ERRORS:
        backend, library = "other", None
    _, directory = runtime_directory_for(environment, backend)
    if directory:
        # Both the child and the short-lived management command must contact the same service.
        environment["XDG_RUNTIME_DIR"] = os.environ["XDG_RUNTIME_DIR"] = directory
    controller = None
    if backend == "monado":
        controller = Monado(NAME)
    elif backend == "wivrn" and library:
        controller = WiVRnControl(library, NAME)
    options = arguments[:arguments.index("--")] if "--" in arguments else arguments
    runs_session = options[:1] != ["ctl"] and not {"--headless", "--list-demos", "-h", "--help"}.intersection(options)
    if controller and runs_session:
        try:
            clients = controller.clients()
        except CONTROL_ERRORS:
            controller = None
    else:
        controller = None
    if controller is None:
        os.execvpe(command[0], command, environment)

    previous = next((c for c in clients if c.flags & PRIMARY and c.flags & ACTIVE), None)
    existing = cinema(clients)
    child = subprocess.Popen(command, env=environment)
    signal.signal(signal.SIGTERM, lambda sig, _: child.send_signal(sig))
    shown = None
    try:
        # A second launch forwards its file over the existing player's control socket, then
        # exits. Promote that session once, leaving its original launcher to manage its lifetime.
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            code = child.poll()
            if code is not None:
                if code != 0 or existing is None:
                    return code
                current = cinema(controller.clients())
                if current and current.id == existing.id:
                    controller.show(current)
                return code
            target = cinema(controller.clients())
            if target and (existing is None or target.id != existing.id):
                controller.show(target)
                shown = target
                break
            time.sleep(0.15)
    except CONTROL_ERRORS as error:
        print(f"{NAME}: {error}", file=sys.stderr)
    try:
        return child.wait()
    finally:
        try:
            restore(controller, previous, shown)
        except CONTROL_ERRORS as error:
            print(f"{NAME}: {error}", file=sys.stderr)


if __name__ == "__main__":
    raise SystemExit(launch(sys.argv[1:]))
