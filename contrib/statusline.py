#!/usr/bin/env python3
"""A Claude Code status line that feeds Signalbox.

Claude Code runs the status line command after every update, with the session's state as JSON
on stdin. This writes what Signalbox shows beside each session (context use, the 5-hour and
7-day limits, lines changed) to ~/.signalbox/status/<session id>.json, and prints a short line
for Claude Code's own status bar.

Use it in ~/.claude/settings.json:

    "statusLine": {"type": "command", "command": "python3 /path/to/signalbox/contrib/statusline.py"}

Already have a status line you like? Run this from it with the same input, or copy write_status.
"""

import json
import os
import pathlib
import re
import sys
import time


def number(value):
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0


def write_status(data):
    session_id = re.sub(r"[^A-Za-z0-9-]", "", str(data.get("session_id") or ""))
    if not session_id:
        return
    context = data.get("context_window") or {}
    cost = data.get("cost") or {}
    limits = data.get("rate_limits") or {}
    workspace = data.get("workspace") or {}
    status = {
        "session_id": session_id,
        "cwd": str(workspace.get("current_dir") or data.get("cwd") or os.getcwd()),
        "model": (data.get("model") or {}).get("display_name") or "",
        "context": {
            "used_pct": number(context.get("used_percentage")),
            "size": number(context.get("context_window_size")),
        },
        "cost": {
            "duration_ms": number(cost.get("total_duration_ms")),
            "lines_added": number(cost.get("total_lines_added")),
            "lines_removed": number(cost.get("total_lines_removed")),
        },
        "limits": {
            "five_hour": limits.get("five_hour") or None,
            "seven_day": limits.get("seven_day") or None,
        },
        "updated": int(time.time()),
    }
    folder = pathlib.Path.home() / ".signalbox" / "status"
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{session_id}.json"
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(status))
    temp.replace(path)


def main():
    data = json.load(sys.stdin)
    try:
        write_status(data)
    except OSError:
        pass  # The status line still shows, whatever happened to the file.
    model = (data.get("model") or {}).get("display_name") or "Claude"
    used = number((data.get("context_window") or {}).get("used_percentage"))
    print(f"{model} · {used}% context")


if __name__ == "__main__":
    main()
