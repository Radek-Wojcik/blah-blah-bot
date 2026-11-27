"""Model registry, download with progress and offline lookup.

Weights are never bundled. They are downloaded on first use from Hugging Face into the
cache folder given by `BBB_MODELS_DIR`. Steps resolve models with `local_path`, which never
touches the network, so processing works offline once the models are cached.

Every model is pinned to one commit of its repository (`revision`), so a changed or taken
over repository cannot put different files on a machine. Changing a pin is a reviewed change:
see docs/SECURITY-REVIEW.md.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from . import protocol

CacheStatus = Literal["ready", "older", "missing"]


@dataclass(frozen=True)
class ModelSpec:
    key: str
    repo: str
    # The full commit hash of the repository that is downloaded and loaded (40 hex digits).
    revision: str
    # Files to fetch (glob patterns); `None` fetches the whole repository.
    patterns: tuple[str, ...] | None
    gated: bool
    license: str


# Files faster-whisper needs; the rest of the repositories (readme, attributes) is skipped.
WHISPER_FILES = (
    "config.json",
    "model.bin",
    "preprocessor_config.json",
    "tokenizer.json",
    "vocabulary.json",
)

MODELS: dict[str, ModelSpec] = {
    "whisper-turbo": ModelSpec(
        "whisper-turbo",
        "dropbox-dash/faster-whisper-large-v3-turbo",
        "0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf",
        WHISPER_FILES,
        False,
        "MIT",
    ),
    "whisper-large-v3": ModelSpec(
        "whisper-large-v3",
        "Systran/faster-whisper-large-v3",
        "edaa852ec7e145841d8ffdb056a99866b5f0a478",
        WHISPER_FILES,
        False,
        "MIT",
    ),
    "diarization": ModelSpec(
        "diarization",
        "pyannote/speaker-diarization-community-1",
        "3533c8cf8e369892e6b79ff1bf80f7b0286a54ee",
        None,
        True,
        "CC-BY-4.0",
    ),
    "vision": ModelSpec(
        "vision",
        "Qwen/Qwen3-VL-8B-Instruct-GGUF",
        "f982a07559d4a2f6c8744d840bf6fccab30eea96",
        ("Qwen3VL-8B-Instruct-Q4_K_M.gguf", "mmproj-Qwen3VL-8B-Instruct-F16.gguf"),
        False,
        "Apache-2.0",
    ),
}

# Which models a worker step needs.
STEP_MODELS: dict[str, tuple[str, ...]] = {
    "asr": ("whisper-turbo",),
    "diarize": ("diarization",),
    "vision": ("vision",),
}


class ModelMissing(RuntimeError):
    """A model is not in the local cache."""


def cache_dir() -> Path | None:
    env = os.environ.get("BBB_MODELS_DIR")
    return Path(env) if env else None


def _effective_cache_dir() -> Path:
    configured = cache_dir()
    if configured is not None:
        return configured
    hub_cache = os.environ.get("HF_HUB_CACHE", "").strip()
    if hub_cache:
        return Path(hub_cache)
    hf_home = os.environ.get("HF_HOME", "").strip()
    if hf_home:
        return Path(hf_home) / "hub"
    return Path.home() / ".cache" / "huggingface" / "hub"


def _repo_snapshots_dir(spec: ModelSpec, root: Path | None = None) -> Path:
    base = root if root is not None else _effective_cache_dir()
    return base / f"models--{spec.repo.replace('/', '--')}" / "snapshots"


def _inside(path: Path, root: Path) -> bool:
    """True when `path` resolves to a location inside `root` (symlinks followed)."""
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, ValueError, RuntimeError):
        return False


def _snapshot_complete(
    folder: Path, patterns: tuple[str, ...] | None, root: Path | None = None
) -> bool:
    """Whether a snapshot folder holds the model files; never raises on unreadable entries.

    With `root`, files whose symlinks resolve outside the cache are not counted.
    """
    try:
        if folder.is_symlink() or not folder.is_dir():
            return False
        if patterns:
            for p in patterns:
                f = folder / p
                if not f.exists() or (root is not None and not _inside(f, root)):
                    return False
            return True
        return any(root is None or _inside(e, root) for e in folder.iterdir())
    except OSError:
        return False


def asr_key(asr_model: str) -> str:
    return "whisper-large-v3" if asr_model == "large-v3" else "whisper-turbo"


def local_path(key: str) -> Path:
    """Folder of a cached model. Never uses the network.

    `BBB_VISION_DIR` points the vision model at a folder that already holds the two GGUF
    files (for example the models of an existing llama.cpp setup).
    """
    override = os.environ.get("BBB_VISION_DIR")
    if key == "vision" and override:
        return Path(override)

    spec = MODELS[key]
    root = _effective_cache_dir()
    try:
        from huggingface_hub import snapshot_download
    except ImportError:
        folder = _repo_snapshots_dir(spec, root) / spec.revision
        if not folder.is_dir():
            raise ModelMissing(
                f"The model '{key}' ({spec.repo}) is not downloaded. "
                "Download it in Settings > AI Models or run the setup again."
            ) from None
        if spec.patterns:
            missing = [p for p in spec.patterns if not (folder / p).exists()]
            if missing:
                raise ModelMissing(
                    f"The model '{key}' is incomplete: missing {', '.join(missing)}. "
                    "Download it again in Settings > AI Models or run the setup again."
                ) from None
        elif not any(folder.iterdir()):
            raise ModelMissing(
                f"The model '{key}' ({spec.repo}) is not downloaded. "
                "Download it in Settings > AI Models or run the setup again."
            ) from None
        return folder

    try:
        path = snapshot_download(
            spec.repo,
            revision=spec.revision,
            allow_patterns=list(spec.patterns) if spec.patterns else None,
            cache_dir=root,
            local_files_only=True,
        )
    except Exception as e:
        raise ModelMissing(
            f"The model '{key}' ({spec.repo}) is not downloaded. "
            "Download it in Settings > AI Models or run the setup again."
        ) from e
    folder = Path(path)
    if spec.patterns:
        missing = [p for p in spec.patterns if not (folder / p).exists()]
        if missing:
            raise ModelMissing(
                f"The model '{key}' is incomplete: missing {', '.join(missing)}. "
                "Download it again in Settings > AI Models or run the setup again."
            )
    return folder


def is_cached(key: str) -> bool:
    try:
        local_path(key)
        return True
    except (ModelMissing, OSError):
        return False


def cache_status(key: str) -> CacheStatus:
    """Return 'ready' for the pinned revision, 'older' for another cached snapshot, or 'missing'."""
    if is_cached(key):
        return "ready"
    spec = MODELS[key]
    root = _effective_cache_dir()
    snapshots = _repo_snapshots_dir(spec, root)
    try:
        if snapshots.is_symlink() or not snapshots.is_dir():
            return "missing"
        entries = list(snapshots.iterdir())
    except OSError:
        return "missing"
    for entry in entries:
        try:
            if entry.name == spec.revision or entry.is_symlink() or not entry.is_dir():
                continue
        except OSError:
            continue
        if _snapshot_complete(entry, spec.patterns, root):
            return "older"
    return "missing"


def download(key: str, token: str | None) -> Path:
    """Download one model, reporting progress as protocol events."""
    from huggingface_hub import snapshot_download
    from tqdm.auto import tqdm

    spec = MODELS[key]
    if spec.gated and not token:
        raise ModelMissing(
            f"'{spec.repo}' is gated: accept its conditions on Hugging Face and add your "
            "access token in Settings > AI Models or in the setup."
        )

    class Reporter(tqdm):  # type: ignore[misc]
        def update(self, n=1):
            super().update(n)
            if self.total and self.unit == "B":
                protocol.progress(
                    "models", self.n / self.total, f"{key}: {self.n >> 20} of {self.total >> 20} MB"
                )

    path = snapshot_download(
        spec.repo,
        revision=spec.revision,
        allow_patterns=list(spec.patterns) if spec.patterns else None,
        cache_dir=cache_dir(),
        token=token,
        tqdm_class=Reporter,
    )
    return Path(path)
