"""Reference voice clips for cloning, rendered locally with Piper (en_US-libritts-high).

The Piper model has 904 LibriTTS speakers (CC BY 4.0). `select_voices` scans a random subset,
keeps clean-sounding candidates and picks the most mutually distant voices per gender;
`build_refs` renders a ~10 s clip for each chosen voice. Piper is used only to make references.
"""

from __future__ import annotations

import json
import random
from pathlib import Path

import numpy as np

from . import audio, paths
from .voices import CATALOG_PATH

PIPER_REPO = "rhasspy/piper-voices"
PIPER_MODEL_FILE = "en/en_US/libritts/high/en_US-libritts-high.onnx"
PIPER_MODEL_SOURCE = f"https://huggingface.co/{PIPER_REPO}/tree/main/en/en_US/libritts/high"
SCAN_TEXT = "The supplier confirmed the delivery date, so we can plan the next test."
REF_TEXT = (
    "Thanks everyone for joining. Last week we finished the first prototype, "
    "and today I would like to walk you through the schedule, the budget and the open questions."
)
PER_GENDER = 10
FEMALE_F0_MIN = 165.0
MALE_F0_MAX = 150.0


def load_piper_voice():
    """Download (once) and load the Piper model."""
    from huggingface_hub import hf_hub_download
    from piper import PiperVoice

    root = paths.piper_dir()
    root.mkdir(parents=True, exist_ok=True)
    for name in (PIPER_MODEL_FILE, PIPER_MODEL_FILE + ".json"):
        hf_hub_download(PIPER_REPO, name, local_dir=str(root))
    return PiperVoice.load(root / PIPER_MODEL_FILE, use_cuda=False)


def render(voice, text: str, speaker_index: int) -> tuple[np.ndarray, int]:
    from piper import SynthesisConfig

    cfg = SynthesisConfig(speaker_id=speaker_index, normalize_audio=True)
    chunks = [c.audio_float_array for c in voice.synthesize(text, syn_config=cfg)]
    return np.concatenate(chunks).astype(np.float32), voice.config.sample_rate


def describe(wave: np.ndarray, rate: int) -> np.ndarray | None:
    """Feature vector of a clip: median f0, spectral centroid and MFCC means; None if unvoiced."""
    import librosa

    f0 = librosa.yin(wave, fmin=60, fmax=400, sr=rate, frame_length=2048)
    rms = librosa.feature.rms(y=wave, frame_length=2048, hop_length=512)[0]
    n = min(len(f0), len(rms))
    voiced = f0[:n][rms[:n] > 0.25 * rms[:n].max()]
    if len(voiced) < 20:
        return None
    centroid = float(librosa.feature.spectral_centroid(y=wave, sr=rate).mean())
    mfcc = librosa.feature.mfcc(y=wave, sr=rate, n_mfcc=13).mean(axis=1)[1:]
    return np.concatenate([[np.median(voiced), centroid], mfcc])


def farthest_points(features: np.ndarray, count: int) -> list[int]:
    """Greedy k-center: start near the middle, then repeatedly add the most distant point."""
    z = (features - features.mean(axis=0)) / (features.std(axis=0) + 1e-9)
    chosen = [int(np.argmin(np.linalg.norm(z, axis=1)))]
    dist = np.linalg.norm(z - z[chosen[0]], axis=1)
    while len(chosen) < count:
        nxt = int(np.argmax(dist))
        chosen.append(nxt)
        dist = np.minimum(dist, np.linalg.norm(z - z[nxt], axis=1))
    return chosen


def select_voices(candidates: int = 260, seed: int = 20260101) -> list[dict]:
    voice = load_piper_voice()
    id_map = voice.config.speaker_id_map
    names = sorted(id_map, key=lambda k: id_map[k])
    picked = random.Random(seed).sample(names, min(candidates, len(names)))
    rows, feats, durations = [], [], []
    for i, name in enumerate(picked):
        wave, rate = render(voice, SCAN_TEXT, id_map[name])
        f = describe(wave, rate)
        if f is not None:
            rows.append(name)
            feats.append(f)
            durations.append(len(wave) / rate)
        if (i + 1) % 20 == 0:
            print(f"scanned {i + 1}/{len(picked)}", flush=True)
    feats_a = np.array(feats)
    durations_a = np.array(durations)
    median_d = float(np.median(durations_a))
    z = np.abs((feats_a - feats_a.mean(axis=0)) / (feats_a.std(axis=0) + 1e-9))
    clean = (np.abs(durations_a / median_d - 1) < 0.25) & (z.max(axis=1) < 2.8)
    result = []
    for gender, mask in (
        ("f", feats_a[:, 0] >= FEMALE_F0_MIN),
        ("m", feats_a[:, 0] <= MALE_F0_MAX),
    ):
        idx = np.flatnonzero(mask & clean)
        for rank, j in enumerate(farthest_points(feats_a[idx], PER_GENDER), start=1):
            k = int(idx[j])
            result.append(
                {
                    "id": f"{gender}{rank:02d}",
                    "gender": gender,
                    "piper_speaker": rows[k],
                    "median_f0_hz": round(float(feats_a[k, 0]), 1),
                    "centroid_hz": round(float(feats_a[k, 1]), 1),
                }
            )
    catalog = {
        "source_model": "en_US-libritts-high",
        "source_url": PIPER_MODEL_SOURCE,
        "license": "CC BY 4.0 (LibriTTS speakers)",
        "selection_seed": seed,
        "voices": result,
    }
    CATALOG_PATH.write_text(json.dumps(catalog, indent=1) + "\n", "utf-8")
    return result


def build_refs(voices: list[dict], out_dir: Path | None = None) -> list[Path]:
    voice = load_piper_voice()
    id_map = voice.config.speaker_id_map
    out_dir = out_dir or paths.refs_dir()
    written = []
    for v in voices:
        wave, rate = render(voice, REF_TEXT, id_map[v["piper_speaker"]])
        wave = audio.match_level(audio.resample(wave, rate, 24000))
        path = out_dir / f"{v['id']}.wav"
        audio.write_wav(path, wave, 24000)
        written.append(path)
    return written
