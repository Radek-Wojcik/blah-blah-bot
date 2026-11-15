"""Line-based progress protocol between the worker and the application.

The worker writes one JSON object per line to stdout. Anything else it prints goes to
stderr. The application reads `progress`, `done` and `error` events; the exit code is 0 on
success and 1 on failure.
"""

from __future__ import annotations

import json
import sys
from typing import Any

_OUT = sys.stdout


def emit(event: str, **fields: Any) -> None:
    _OUT.write(json.dumps({"event": event, **fields}, ensure_ascii=False) + "\n")
    _OUT.flush()


def progress(step: str, fraction: float, message: str = "") -> None:
    emit("progress", step=step, fraction=round(max(0.0, min(1.0, fraction)), 4), message=message)


def done(step: str, **fields: Any) -> None:
    emit("done", step=step, **fields)


def error(step: str, message: str) -> None:
    emit("error", step=step, message=message)
