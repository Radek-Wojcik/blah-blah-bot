"""Model registry, download with progress and offline lookup.

Weights are never bundled. They are downloaded on first use from Hugging Face into the
cache folder given by `BBB_MODELS_DIR`. Steps resolve models with `local_path`, which never
touches the network, so processing works offline once the models are cached.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from . import protocol


@dataclass(frozen=True)
class ModelSpec:
    key: str
    repo: str
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
        "whisper-turbo", "mobiuslabsgmbh/faster-whisper-large-v3-turbo", WHISPER_FILES, False, "MIT"
    ),
    "whisper-large-v3": ModelSpec(
        "whisper-large-v3", "Systran/faster-whisper-large-v3", WHISPER_FILES, False, "MIT"
    ),
    "diarization": ModelSpec(
        "diarization", "pyannote/speaker-diarization-community-1", None, True, "CC-BY-4.0"
    ),
    "vision": ModelSpec(
        "vision",
        "Qwen/Qwen3-VL-8B-Instruct-GGUF",
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


def asr_key(asr_model: str) -> str:
    return "whisper-large-v3" if asr_model == "large-v3" else "whisper-turbo"


def local_path(key: str) -> Path:
    """Folder of a cached model. Never uses the network.

    `BBB_VISION_DIR` points the vision model at a folder that already holds the two GGUF
    files (for example the models of an existing llama.cpp setup).
    """
    from huggingface_hub import snapshot_download

    override = os.environ.get("BBB_VISION_DIR")
    if key == "vision" and override:
        return Path(override)

    spec = MODELS[key]
    try:
        path = snapshot_download(
            spec.repo,
            allow_patterns=list(spec.patterns) if spec.patterns else None,
            cache_dir=cache_dir(),
            local_files_only=True,
        )
    except Exception as e:
        raise ModelMissing(
            f"The model '{key}' ({spec.repo}) is not downloaded. "
            "Download it in Settings > Processing or run the setup again."
        ) from e
    folder = Path(path)
    if spec.patterns:
        missing = [p for p in spec.patterns if not (folder / p).exists()]
        if missing:
            raise ModelMissing(
                f"The model '{key}' is incomplete: missing {', '.join(missing)}. "
                "Download it again in Settings > Processing or run the setup again."
            )
    return folder


def is_cached(key: str) -> bool:
    try:
        local_path(key)
        return True
    except ModelMissing:
        return False


def download(key: str, token: str | None) -> Path:
    """Download one model, reporting progress as protocol events."""
    from huggingface_hub import snapshot_download
    from tqdm.auto import tqdm

    spec = MODELS[key]
    if spec.gated and not token:
        raise ModelMissing(
            f"'{spec.repo}' is gated: accept its conditions on Hugging Face and add your "
            "access token in Settings > Processing or in the setup."
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
        allow_patterns=list(spec.patterns) if spec.patterns else None,
        cache_dir=cache_dir(),
        token=token,
        tqdm_class=Reporter,
    )
    return Path(path)
