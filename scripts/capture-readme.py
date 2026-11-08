#!/usr/bin/env python3
"""Capture Roughcut's real UI using disposable demo data. Requires macOS and Rust."""

import argparse
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SCREENS = {
    "01-home": "library",
    "02-editor-page": "page",
    "03-editor-board": "board",
    "06-focus": "focus",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/images", help="destination (default: docs/images)")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("UI capture requires macOS with a graphical login session")

    # Ask Cargo for the executable path so custom target directories also work.
    print("Building Roughcut with screenshot support…", flush=True)
    build = subprocess.run(
        ["cargo", "build", "-p", "quill", "--features", "screenshots", "--message-format=json"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    executable = None
    for line in build.stdout.splitlines():
        event = json.loads(line)
        if event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == "roughcut":
            executable = event.get("executable") or executable
    if not executable:
        raise RuntimeError("Cargo did not report the Roughcut executable")

    with tempfile.TemporaryDirectory(prefix="roughcut-readme-") as scratch:
        scratch = Path(scratch)
        shots = scratch / "shots"
        env = os.environ.copy()
        env.update(ROUGHCUT_HOME=str(scratch / "library"), ROUGHCUT_DEMO="1", ROUGHCUT_SCREENSHOTS=str(shots))
        print("Capturing the demo library, page, whiteboard, and focus view…", flush=True)
        subprocess.run([executable], cwd=ROOT, env=env, check=True, timeout=180)
        staged = scratch / "ready"
        staged.mkdir()
        for source, name in SCREENS.items():
            source = shots / f"{source}.png"
            data = source.read_bytes()
            if data[:8] != b"\x89PNG\r\n\x1a\n" or len(data) < 24:
                raise RuntimeError(f"Invalid screenshot: {source.name}")
            width, height = struct.unpack(">II", data[16:24])
            if width < 1000 or height < 600:
                raise RuntimeError(f"Unexpected screenshot dimensions: {width}x{height}")
            subprocess.run(
                ["sips", "-Z", "1440", str(source), "--out", str(staged / f"{name}.png")],
                check=True, stdout=subprocess.DEVNULL,
            )
        # Publish only after every requested screen was captured and validated.
        args.output.mkdir(parents=True, exist_ok=True)
        for name in SCREENS.values():
            shutil.copyfile(staged / f"{name}.png", args.output / f"{name}.png")
        print(f"Updated {len(SCREENS)} screenshots in {args.output}")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        if error.stderr:
            print(error.stderr, file=sys.stderr)
        sys.exit(f"Screenshot capture failed: {error}")
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        sys.exit(f"Screenshot capture failed: {error}")
