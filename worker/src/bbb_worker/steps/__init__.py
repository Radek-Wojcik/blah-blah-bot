"""Worker steps. Each step reads the manifest and earlier outputs from the work folder and
writes its own outputs there, so a crashed step can simply be run again."""

from __future__ import annotations

import contextlib
import json
import os
import tempfile
from pathlib import Path
from typing import Any


def write_bytes(path: Path, data: bytes) -> None:
    """Write atomically through a uniquely named temporary file in the same folder, so that a
    crash never leaves a half-written output and two writers never share a temporary file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=f"{path.name}.", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
        os.replace(tmp_name, path)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp_name)
        raise


def write_json(path: Path, doc: Any) -> bytes:
    """Write `doc` atomically; returns the exact bytes that were written."""
    data = json.dumps(doc, ensure_ascii=False, indent=1, allow_nan=False).encode("utf-8")
    write_bytes(path, data)
    return data


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))
