"""Small audio helpers: resampling, silence trimming, level matching and WAV output."""

from __future__ import annotations

from math import gcd
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

OUT_RATE = 16000
TARGET_RMS = 0.07  # about -23 dBFS


def resample(wave: np.ndarray, rate_in: int, rate_out: int) -> np.ndarray:
    if rate_in == rate_out:
        return wave.astype(np.float32, copy=False)
    g = gcd(rate_in, rate_out)
    return resample_poly(wave, rate_out // g, rate_in // g).astype(np.float32)


def trim_silence(
    wave: np.ndarray, rate: int, threshold_db: float = -40.0, pad_s: float = 0.04
) -> np.ndarray:
    """Cut leading and trailing silence so the turn boundaries match the speech."""
    frame = max(int(rate * 0.01), 1)
    n = len(wave) // frame
    if n == 0:
        return wave
    rms = np.sqrt(np.mean(wave[: n * frame].reshape(n, frame) ** 2, axis=1))
    peak = rms.max()
    if peak <= 0:
        return wave
    active = np.flatnonzero(rms >= peak * 10 ** (threshold_db / 20))
    pad = int(pad_s * rate)
    start = max(active[0] * frame - pad, 0)
    end = min((active[-1] + 1) * frame + pad, len(wave))
    return wave[start:end]


def match_level(wave: np.ndarray, target_rms: float = TARGET_RMS) -> np.ndarray:
    """Scale to a common RMS (so voices are similarly loud) without clipping."""
    rms = float(np.sqrt(np.mean(wave**2))) if len(wave) else 0.0
    if rms <= 0:
        return wave
    out = wave * (target_rms / rms)
    peak = float(np.abs(out).max())
    if peak > 0.97:
        out = out * (0.97 / peak)
    return out.astype(np.float32)


def write_wav(path: Path, wave: np.ndarray, rate: int = OUT_RATE) -> None:
    """16-bit PCM mono WAV."""
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(path), np.clip(wave, -1.0, 1.0), rate, subtype="PCM_16")


def read_wav(path: Path) -> tuple[np.ndarray, int]:
    data, rate = sf.read(str(path), dtype="float32", always_2d=False)
    if data.ndim > 1:
        data = data.mean(axis=1)
    return data, rate
