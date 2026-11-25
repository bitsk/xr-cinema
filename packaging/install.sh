#!/bin/sh
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: sh install.sh [XR_CINEMA_BINARY]" >&2
    exit 2
fi
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -eq 1 ]; then
    binary=$1
else
    target_dir=${CARGO_TARGET_DIR:-"$root/target"}
    cargo build --manifest-path "$root/Cargo.toml" --target-dir "$target_dir" --release --locked
    binary=$target_dir/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}release/xr-cinema
fi
test -f "$binary"
python3 - "$root/assets" <<'PY'
import json
from pathlib import Path
import sys

assets = Path(sys.argv[1])
catalog = json.loads((assets / "demos/catalog.json").read_text(encoding="utf-8"))
for item in catalog:
    if not (assets / item["file"]).is_file():
        raise SystemExit(f"missing demo: {item['file']}")
for notice in ("flat-SOURCES.md", "panorama-SOURCES.md", "immersive-SOURCES.md"):
    if not (assets / "demos" / notice).is_file():
        raise SystemExit(f"missing media attribution: {notice}")
PY
dest=${DESTDIR:-}${PREFIX:-/usr/local}
install -Dm755 "$binary" "$dest/bin/xr-cinema"
install -Dm644 "$root/assets/环行星.webm" "$dest/share/xr-cinema/环行星.webm"
install -d "$dest/share/xr-cinema/demos"
install -m644 "$root"/assets/demos/*.mp4 "$root"/assets/demos/*.md "$root/assets/demos/catalog.json" "$dest/share/xr-cinema/demos/"
install -Dm755 "$root/packaging/launch.py" "$dest/lib/xr-cinema/launch.py"
install -Dm644 "$root/packaging/runtime_control.py" "$dest/lib/xr-cinema/runtime_control.py"
ln -sfn ../lib/xr-cinema/launch.py "$dest/bin/xr-cinema-desktop"
install -Dm644 "$root/packaging/org.openkylin.XRCinema.desktop" \
    "$dest/share/applications/org.openkylin.XRCinema.desktop"
install -Dm644 "$root/packaging/xr-cinema.json" "$dest/share/xr/applications/xr-cinema.json"
