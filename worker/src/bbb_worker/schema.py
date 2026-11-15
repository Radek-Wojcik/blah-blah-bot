"""Validation against the JSON Schemas in `schemas/`."""

from __future__ import annotations

import copy
import json
import os
from functools import cache
from pathlib import Path
from typing import Any

import jsonschema


def schemas_dir() -> Path:
    env = os.environ.get("BBB_SCHEMAS_DIR")
    if env:
        return Path(env)
    # worker/src/bbb_worker/schema.py -> repository root / schemas
    return Path(__file__).resolve().parents[3] / "schemas"


@cache
def _load(name: str) -> dict[str, Any]:
    return json.loads((schemas_dir() / f"{name}.schema.json").read_text(encoding="utf-8"))


def load(name: str) -> dict[str, Any]:
    """A copy of the schema document, for sending to a model as a response format."""
    return copy.deepcopy(_load(name))


@cache
def _validator(name: str) -> jsonschema.Draft202012Validator:
    schema = _load(name)
    jsonschema.Draft202012Validator.check_schema(schema)
    return jsonschema.Draft202012Validator(schema)


def validate(name: str, document: Any) -> None:
    """Raise `jsonschema.ValidationError` with a readable path when the document is invalid."""
    errors = sorted(_validator(name).iter_errors(document), key=lambda e: list(e.path))
    if errors:
        e = errors[0]
        where = "/".join(str(p) for p in e.path) or "<root>"
        raise jsonschema.ValidationError(f"{name}: {where}: {e.message}")
