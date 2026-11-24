"""Manual 15-30 s voice enrollment: decode audio, extract cleanest 4-6 s voiced preview clip,
and compute a 256-dim L2-normalized speaker embedding."""

from __future__ import annotations

import json
import os
import wave
from pathlib import Path
from typing import Any

import numpy as np

from . import models, protocol
from .manifest import SAMPLE_RATE, Manifest
from .steps import write_json

STEP = "enroll"
EMBEDDING_DIM = 256
MIN_CLIP_SECONDS = 4.0
MAX_CLIP_SECONDS = 6.0
DEFAULT_CLIP_SECONDS = 5.0
FRAME_MS = 100
FRAME_SAMPLES = (SAMPLE_RATE * FRAME_MS) // 1000  # 1600 samples


def _read_wav_any(path: Path) -> np.ndarray | None:
    """Fast path for uncompressed PCM WAV files (mono or multi-channel, any integer rate)."""
    if path.suffix.lower() != ".wav":
        return None
    try:
        with wave.open(str(path), "rb") as w:
            if w.getcomptype() != "NONE" or w.getsampwidth() != 2:
                return None
            channels = w.getnchannels()
            rate = w.getframerate()
            raw = w.readframes(w.getnframes())
    except (OSError, wave.Error):
        return None

    data = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    if channels > 1:
        frames = len(data) // channels
        data = data[: frames * channels].reshape(frames, channels).mean(axis=1)
    if rate != SAMPLE_RATE and len(data) > 0:
        out_len = max(1, round(len(data) * SAMPLE_RATE / rate))
        src_x = np.linspace(0.0, 1.0, len(data), endpoint=False)
        dst_x = np.linspace(0.0, 1.0, out_len, endpoint=False)
        data = np.interp(dst_x, src_x, data).astype(np.float32)
    return data.astype(np.float32, copy=False)


def decode_audio_16k_mono(input_path: Path) -> np.ndarray:
    """Decode an audio or video sample file to 16 kHz mono float32 in `[-1.0, 1.0]`."""
    input_path = Path(input_path)
    if not input_path.exists():
        raise FileNotFoundError(f"enrollment input file not found: {input_path}")

    fast = _read_wav_any(input_path)
    if fast is not None:
        if fast.size == 0:
            raise ValueError(f"{input_path.name}: audio file is empty")
        return fast

    import av

    chunks: list[np.ndarray] = []
    with av.open(str(input_path)) as container:
        if not container.streams.audio:
            raise ValueError(f"{input_path.name}: no audio stream found")
        resampler = av.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)
        for frame in container.decode(audio=0):
            for rframe in resampler.resample(frame):
                arr = rframe.to_ndarray().reshape(-1).astype(np.float32) / 32768.0
                if arr.size > 0:
                    chunks.append(arr)
        for rframe in resampler.resample(None):
            arr = rframe.to_ndarray().reshape(-1).astype(np.float32) / 32768.0
            if arr.size > 0:
                chunks.append(arr)

    if not chunks:
        raise ValueError(f"{input_path.name}: decoded 0 audio samples")
    return np.concatenate(chunks).astype(np.float32, copy=False)


def extract_cleanest_clip(
    audio: np.ndarray,
    *,
    sample_rate: int = SAMPLE_RATE,
    min_seconds: float = MIN_CLIP_SECONDS,
    max_seconds: float = MAX_CLIP_SECONDS,
    target_seconds: float = DEFAULT_CLIP_SECONDS,
) -> tuple[np.ndarray, int, int]:
    """Select the cleanest 4-6 s voiced window from `audio`.
    Returns `(clip_samples, start_ms, clip_duration_ms)`."""
    if audio.size == 0:
        raise ValueError("cannot extract preview clip from empty audio")
    if not np.isfinite(audio).all():
        raise ValueError("audio contains non-finite samples")

    clamped_s = min(max_seconds, max(min_seconds, target_seconds))
    target_samples = min(len(audio), round(clamped_s * sample_rate))
    min_samples = round(min_seconds * sample_rate)
    if len(audio) <= min_samples or len(audio) <= target_samples:
        duration_ms = round(len(audio) * 1000.0 / sample_rate)
        return audio.copy(), 0, duration_ms

    step = max(1, (sample_rate * FRAME_MS) // 1000)
    num_frames = len(audio) // step
    if num_frames == 0:
        duration_ms = round(target_samples * 1000.0 / sample_rate)
        return audio[:target_samples].copy(), 0, duration_ms

    trimmed = audio[: num_frames * step].reshape(num_frames, step)
    rms = np.sqrt(np.mean(trimmed * trimmed, axis=1) + 1e-12)
    peaks = np.max(np.abs(trimmed), axis=1)
    noise_floor = max(0.005, float(np.percentile(rms, 25)) * 1.2)
    voiced = (rms > noise_floor) & (peaks < 0.99)
    clipped = peaks >= 0.995

    window_frames = max(1, target_samples // step)
    best_score = -1e9
    best_start_frame = 0

    for start_f in range(max(1, num_frames - window_frames + 1)):
        end_f = start_f + window_frames
        win_rms = rms[start_f:end_f]
        win_voiced = voiced[start_f:end_f]
        win_clipped = clipped[start_f:end_f]
        voiced_ratio = float(np.mean(win_voiced))
        mean_energy = float(np.mean(win_rms))
        clip_penalty = float(np.mean(win_clipped)) * 2.0
        # Prefer consistent speech energy without sudden silence gaps or clipping.
        energy_std = float(np.std(win_rms))
        consistency = mean_energy / (energy_std + 0.02)
        score = voiced_ratio * 2.0 + mean_energy * 3.0 + 0.2 * consistency - clip_penalty
        if score > best_score:
            best_score = score
            best_start_frame = start_f

    start_sample = min(best_start_frame * step, len(audio) - target_samples)
    clip = audio[start_sample : start_sample + target_samples].copy()
    start_ms = round(start_sample * 1000.0 / sample_rate)
    clip_duration_ms = round(len(clip) * 1000.0 / sample_rate)
    return clip, start_ms, clip_duration_ms


def write_wav_16k_mono(path: Path, audio: np.ndarray) -> None:
    """Write float32 audio in `[-1.0, 1.0]` as a 16 kHz mono 16-bit PCM WAV file."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcm = np.clip(np.round(audio * 32767.0), -32768, 32767).astype("<i2")
    with wave.open(str(path), "wb") as dst:
        dst.setnchannels(1)
        dst.setsampwidth(2)
        dst.setframerate(SAMPLE_RATE)
        dst.writeframes(pcm.tobytes())


def normalize_embedding(vec: Any, dim: int = EMBEDDING_DIM) -> list[float]:
    """Validate finiteness, coerce to `dim` floats, and L2-normalize."""
    arr = np.asarray(vec, dtype=np.float32).reshape(-1)
    if arr.size == 0 or not np.isfinite(arr).all():
        raise ValueError("speaker embedding contains non-finite or empty values")
    if arr.size < dim:
        padded = np.zeros(dim, dtype=np.float32)
        padded[: arr.size] = arr
        arr = padded
    elif arr.size > dim:
        arr = arr[:dim]
    norm = float(np.linalg.norm(arr))
    if not np.isfinite(norm) or norm < 1e-8:
        raise ValueError("speaker embedding has zero or non-finite L2 norm")
    normalized = (arr / norm).astype(np.float32)
    if not np.isfinite(normalized).all():
        raise ValueError("normalized speaker embedding contains non-finite values")
    return [float(x) for x in normalized.tolist()]


def deterministic_embedding(audio: np.ndarray, dim: int = EMBEDDING_DIM) -> list[float]:
    """Deterministic 256-dim L2-normalized acoustic embedding fallback for CPU unit tests."""
    arr = np.asarray(audio, dtype=np.float32).reshape(-1)
    if arr.size == 0 or not np.isfinite(arr).all():
        raise ValueError("cannot compute embedding from empty or non-finite audio")
    if float(np.max(np.abs(arr))) < 1e-6:
        raise ValueError("cannot compute speaker embedding from silent audio")

    # Frame the signal into 32 ms windows (512 samples) with 16 ms hop (256 samples).
    win_len = 512
    hop = 256
    if len(arr) < win_len:
        arr = np.pad(arr, (0, win_len - len(arr)), mode="constant")
    num_frames = 1 + (len(arr) - win_len) // hop
    window = np.hanning(win_len).astype(np.float32)
    frames = np.stack([arr[i * hop : i * hop + win_len] * window for i in range(num_frames)])

    # RFFT magnitude spectrum has 257 bins; take bins 1..129 (128 bins) mean and std -> 256 dims.
    mag = np.abs(np.fft.rfft(frames, axis=1))[:, 1 : 1 + (dim // 2)]
    log_mag = np.log1p(mag)
    mean_feat = np.mean(log_mag, axis=0)
    std_feat = np.std(log_mag, axis=0)
    raw = np.concatenate([mean_feat, std_feat]).astype(np.float32)
    return normalize_embedding(raw, dim=dim)


def _pyannote_embedding(audio: np.ndarray) -> list[float] | None:
    """Compute 256-dim speaker embedding using the cached pyannote diarization model if available."""
    if os.environ.get("BBB_ENROLL_FALLBACK") == "1":
        return None
    try:
        path = models.local_path("diarization")
        import torch
        from pyannote.audio import Pipeline
    except Exception:  # noqa: BLE001
        return None

    try:
        pipeline = Pipeline.from_pretrained(str(path))
        device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
        pipeline.to(device)
        waveform = torch.from_numpy(audio.astype(np.float32, copy=False))[None, :]
        if hasattr(pipeline, "_embedding") and callable(pipeline._embedding):
            emb = pipeline._embedding(waveform[None, :].to(device))
            return normalize_embedding(np.asarray(emb, dtype=np.float32))
        out = pipeline({"waveform": waveform, "sample_rate": SAMPLE_RATE}, max_speakers=1)
        if out.speaker_embeddings is not None and len(out.speaker_embeddings) > 0:
            return normalize_embedding(out.speaker_embeddings[0])
    except Exception:  # noqa: BLE001
        return None
    return None


def compute_speaker_embedding(audio: np.ndarray, *, allow_fallback: bool = True) -> list[float]:
    """Compute a 256-dim L2-normalized speaker embedding from 16 kHz mono float32 audio."""
    emb = _pyannote_embedding(audio)
    if emb is not None:
        return emb
    if not allow_fallback:
        raise RuntimeError("cached pyannote diarization embedding model is unavailable")
    return deterministic_embedding(audio)


def enroll_voice(
    input_path: Path,
    clip_out_path: Path,
    output_path: Path,
    *,
    allow_fallback: bool = True,
) -> dict[str, Any]:
    """Decode a 15-30 s voice sample, write the cleanest 4-6 s preview clip to `clip_out_path`,
    compute the 256-dim L2-normalized embedding, and write the result JSON to `output_path`."""
    input_path = Path(input_path)
    clip_out_path = Path(clip_out_path)
    output_path = Path(output_path)

    protocol.progress(STEP, 0.05, f"decoding {input_path.name}")
    audio = decode_audio_16k_mono(input_path)
    duration_ms = round(len(audio) * 1000.0 / SAMPLE_RATE)

    protocol.progress(STEP, 0.35, "extracting preview clip")
    clip, _start_ms, _clip_duration_ms = extract_cleanest_clip(audio)
    write_wav_16k_mono(clip_out_path, clip)

    protocol.progress(STEP, 0.65, "computing speaker embedding")
    embedding = compute_speaker_embedding(clip, allow_fallback=allow_fallback)

    doc = {
        "embedding": embedding,
        "duration_ms": duration_ms,
        "clip_file": str(clip_out_path),
    }
    write_json(output_path, doc)
    protocol.progress(STEP, 1.0, f"enrolled ({duration_ms} ms)")
    return doc


def run(manifest: Manifest | dict[str, Any], meeting_dir: Path | None = None) -> dict[str, Any]:
    """Entry point for `enroll` step when invoked via manifest or dict."""
    if isinstance(manifest, Manifest):
        m_dir = meeting_dir or manifest.meeting_dir
        options = manifest.options
    else:
        m_dir = Path(meeting_dir or manifest.get("meeting_dir", "."))
        options = manifest.get("options", manifest)

    enroll_opts = options.get("enroll") if isinstance(options.get("enroll"), dict) else options
    input_raw = enroll_opts.get("input_path") or enroll_opts.get("input")
    if not input_raw:
        raise ValueError("enroll requires input_path")

    work_dir = m_dir / "work"
    clip_raw = (
        enroll_opts.get("clip_out_path")
        or enroll_opts.get("clip_out")
        or enroll_opts.get("clip_file")
        or (work_dir / "enroll-clip.wav")
    )
    out_raw = (
        enroll_opts.get("output_path") or enroll_opts.get("output") or (work_dir / "enroll.json")
    )
    return enroll_voice(Path(input_raw), Path(clip_raw), Path(out_raw))


def load_enroll_manifest(manifest_path: Path, meeting_dir: Path | None = None) -> dict[str, Any]:
    """Load a manifest JSON for `enroll` without rejecting `options.enroll`."""
    doc = json.loads(manifest_path.read_text(encoding="utf-8"))
    if meeting_dir is not None:
        doc.setdefault("meeting_dir", str(meeting_dir))
    return doc
