"""Chatterbox Multilingual wrapper: voice cloning from a short reference clip (MIT)."""

from __future__ import annotations

from pathlib import Path

import numpy as np

# Speaking rate bounds (words per second) used to detect failed generations.
MIN_WPS = 0.8
MAX_WPS = 6.0


def plausible(text: str, duration_s: float) -> bool:
    """False when the audio length is implausible for the text (silence, truncation or babble)."""
    words = max(len(text.split()), 1)
    return words / MAX_WPS * 0.6 <= duration_s <= words / MIN_WPS + 1.5


class ChatterboxEngine:
    """Loads the model once; speaks text in a given language with a cloned reference voice."""

    def __init__(self, ref_dir: Path, device: str = "cuda") -> None:
        import torch
        from chatterbox.mtl_tts import ChatterboxMultilingualTTS

        self._torch = torch
        self.device = device
        self.ref_dir = ref_dir
        self.model = ChatterboxMultilingualTTS.from_pretrained(device=device)
        self.sample_rate: int = self.model.sr
        self._conds: dict[str, object] = {}

    def _conditionals(self, voice_id: str):
        if voice_id not in self._conds:
            path = self.ref_dir / f"{voice_id}.wav"
            if not path.exists():
                raise FileNotFoundError(f"missing reference clip {path}; run build-refs")
            self.model.prepare_conditionals(str(path))
            self._conds[voice_id] = self.model.conds
        return self._conds[voice_id]

    def synth(self, text: str, lang: str, voice_id: str, seed: int) -> np.ndarray:
        """Return mono float32 audio at `self.sample_rate`."""
        torch = self._torch
        torch.manual_seed(seed)
        if self.device.startswith("cuda"):
            torch.cuda.manual_seed_all(seed)
        self.model.conds = self._conditionals(voice_id)
        # cfg_weight 0 reduces accent carry-over when the reference clip is in another language.
        cfg = 0.5 if lang == "en" else 0.0
        wav = self.model.generate(text, language_id=lang, cfg_weight=cfg)
        return wav.squeeze(0).detach().cpu().numpy().astype(np.float32)

    def peak_vram_mb(self) -> float:
        if not self.device.startswith("cuda"):
            return 0.0
        return self._torch.cuda.max_memory_allocated() / 2**20
