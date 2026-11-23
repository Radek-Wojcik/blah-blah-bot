"""Audio and video media ingest with a cursor-immune slide-change extractor.

Decodes audio or video files (`mp3`, `wav`, `m4a`, `ogg`, `flac`, `aac`, `mkv`, `mp4`, `webm`,
`mov`, `avi`) into 60-second 16 kHz mono 16-bit WAV segments under `tracks/imported/`, and
optionally extracts stable, deduplicated presentation slides at ~1 fps into `img/`.
"""

from __future__ import annotations

import json
import struct
import wave
import zlib
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from . import protocol
from .manifest import SAMPLE_RATE, Manifest
from .steps import write_bytes, write_json

STEP = "ingest"
SEGMENT_SECONDS = 60
SEGMENT_SAMPLES = SAMPLE_RATE * SEGMENT_SECONDS
TRACK_NAME = "imported"

THUMB_WIDTH = 160
THUMB_HEIGHT = 90
GRID_COLS = 8
GRID_ROWS = 6
BLOCK_WIDTH = THUMB_WIDTH // GRID_COLS  # 20
BLOCK_HEIGHT = THUMB_HEIGHT // GRID_ROWS  # 15

BLOCK_DIFF_THRESHOLD = 0.06
MIN_CHANGED_BLOCKS = 5
MAX_UNSTABLE_BLOCKS = 2
STABLE_FRAMES_REQUIRED = 2
COOLDOWN_MS = 5_000.0
DEDUP_COSINE_THRESHOLD = 0.96
MAX_SLIDES = 40
MAX_SLIDE_WIDTH = 1280

AUDIO_EXTENSIONS = frozenset({".mp3", ".wav", ".m4a", ".ogg", ".flac", ".aac"})
VIDEO_EXTENSIONS = frozenset({".mkv", ".mp4", ".webm", ".mov", ".avi"})
SUPPORTED_EXTENSIONS = AUDIO_EXTENSIONS | VIDEO_EXTENSIONS


@dataclass(frozen=True)
class SlideCapture:
    index: int
    offset_ms: int
    file: str
    frame: Any


def _frame_to_rgb_uint8(frame: Any) -> np.ndarray:
    """Convert a Pillow Image, PyAV VideoFrame, or numpy array to an `(H, W, 3)` uint8 array."""
    if hasattr(frame, "to_ndarray"):
        arr = frame.to_ndarray(format="rgb24")
    elif hasattr(frame, "convert"):
        arr = np.asarray(frame.convert("RGB"), dtype=np.uint8)
    else:
        arr = np.asarray(frame)
    if arr.dtype != np.uint8:
        if np.issubdtype(arr.dtype, np.floating) and arr.size > 0 and float(arr.max()) <= 1.0:
            arr = np.clip(arr * 255.0, 0, 255).astype(np.uint8)
        else:
            arr = np.clip(arr, 0, 255).astype(np.uint8)
    if arr.ndim == 2:
        return np.stack([arr, arr, arr], axis=-1)
    if arr.ndim == 3 and arr.shape[2] == 4:
        return arr[:, :, :3]
    if arr.ndim == 3 and arr.shape[2] == 1:
        return np.repeat(arr, 3, axis=2)
    return arr


def _frame_to_gray_float32(frame: Any) -> np.ndarray:
    """Convert a frame to a 2D `(H, W)` float32 grayscale array in `[0.0, 1.0]`."""
    if isinstance(frame, np.ndarray) and frame.ndim == 2:
        if np.issubdtype(frame.dtype, np.floating):
            scale = 255.0 if (frame.size > 0 and float(frame.max()) > 1.0) else 1.0
            return np.clip(frame.astype(np.float32) / scale, 0.0, 1.0)
        return np.clip(frame.astype(np.float32) / 255.0, 0.0, 1.0)
    rgb = _frame_to_rgb_uint8(frame).astype(np.float32) / 255.0
    return 0.299 * rgb[:, :, 0] + 0.587 * rgb[:, :, 1] + 0.114 * rgb[:, :, 2]


def _resize_2d(image: np.ndarray, out_h: int, out_w: int) -> np.ndarray:
    """Area-averaging downscale (or nearest resize) of a 2D float32 array to `(out_h, out_w)`."""
    in_h, in_w = image.shape
    if in_h == out_h and in_w == out_w:
        return image.astype(np.float32, copy=False)
    if in_h >= out_h and in_w >= out_w and in_h % out_h == 0 and in_w % out_w == 0:
        bh = in_h // out_h
        bw = in_w // out_w
        return image.reshape(out_h, bh, out_w, bw).mean(axis=(1, 3)).astype(np.float32)
    row_idx = (np.linspace(0, in_h - 1, out_h)).astype(np.intp)
    col_idx = (np.linspace(0, in_w - 1, out_w)).astype(np.intp)
    return image[np.ix_(row_idx, col_idx)].astype(np.float32)


def box_blur_3x3(gray: np.ndarray) -> np.ndarray:
    """Apply a `3x3` box blur with edge padding to a 2D float32 array."""
    padded = np.pad(gray.astype(np.float32, copy=False), 1, mode="edge")
    return (
        padded[0:-2, 0:-2]
        + padded[0:-2, 1:-1]
        + padded[0:-2, 2:]
        + padded[1:-1, 0:-2]
        + padded[1:-1, 1:-1]
        + padded[1:-1, 2:]
        + padded[2:, 0:-2]
        + padded[2:, 1:-1]
        + padded[2:, 2:]
    ) / 9.0


def to_gray_blurred_160x90(frame: Any) -> np.ndarray:
    """Downscale `frame` to `160x90` grayscale `float32` (`shape == (90, 160)`) + `3x3` box blur."""
    gray = _frame_to_gray_float32(frame)
    small = _resize_2d(gray, THUMB_HEIGHT, THUMB_WIDTH)
    return box_blur_3x3(small)


def block_diff_means(frame_a: np.ndarray, frame_b: np.ndarray) -> np.ndarray:
    """Mean absolute difference for each of the `8x6` = 48 spatial blocks of `20x15` px."""
    diff = np.abs(frame_a - frame_b)
    blocks = diff.reshape(GRID_ROWS, BLOCK_HEIGHT, GRID_COLS, BLOCK_WIDTH)
    return blocks.mean(axis=(1, 3))


def block_diff_count(
    frame_a: np.ndarray,
    frame_b: np.ndarray,
    threshold: float = BLOCK_DIFF_THRESHOLD,
) -> int:
    """Number of `20x15` blocks (out of 48) whose mean absolute diff exceeds `threshold`."""
    return int(np.sum(block_diff_means(frame_a, frame_b) > threshold))


def perceptual_vector_16x16(blurred_90x160: np.ndarray) -> np.ndarray:
    """Compute a 256-dim L2-normalized perceptual vector from a `16x16` downscaled grid."""
    grid = _resize_2d(blurred_90x160, 16, 16).reshape(-1).astype(np.float32)
    centered = grid - float(grid.mean())
    norm = float(np.linalg.norm(centered))
    if norm >= 1e-4:
        return centered / norm
    raw_norm = float(np.linalg.norm(grid))
    if raw_norm >= 1e-6:
        return grid / raw_norm
    out = np.zeros_like(grid)
    out[0] = 1.0
    return out


def cosine_similarity(vec_a: np.ndarray, vec_b: np.ndarray) -> float:
    """Normalized cosine similarity in `[-1.0, 1.0]` between two perceptual vectors."""
    denom = float(np.linalg.norm(vec_a) * np.linalg.norm(vec_b))
    if denom < 1e-8:
        return 1.0
    return float(np.dot(vec_a, vec_b) / denom)


def _encode_png_rgb(rgb: np.ndarray) -> bytes:
    """Minimal pure-Python PNG encoder for an `(H, W, 3)` uint8 array (fallback without Pillow)."""
    height, width, _ = rgb.shape
    raw_rows = b"".join(b"\x00" + rgb[y].tobytes() for y in range(height))

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    idat = zlib.compress(raw_rows, level=6)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def save_slide_png(frame: Any, out_path: Path, max_width: int = MAX_SLIDE_WIDTH) -> None:
    """Save `frame` as a PNG image at `out_path`, downscaling proportionally if wider than 1280px."""
    rgb = _frame_to_rgb_uint8(frame)
    height, width, _ = rgb.shape
    try:
        from PIL import Image

        img = Image.fromarray(rgb, mode="RGB")
        if width > max_width:
            new_h = max(1, round(height * max_width / width))
            img = img.resize((max_width, new_h), Image.Resampling.LANCZOS)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        img.save(out_path, format="PNG")
        return
    except ImportError:
        pass

    if width > max_width:
        new_h = max(1, round(height * max_width / width))
        row_idx = np.linspace(0, height - 1, new_h).astype(np.intp)
        col_idx = np.linspace(0, width - 1, max_width).astype(np.intp)
        rgb = rgb[np.ix_(row_idx, col_idx)]
    write_bytes(out_path, _encode_png_rgb(rgb))


class SlideDetector:
    """Cursor-immune slide-change detector with spatial block-diff, stability debounce,
    cooldown, and `16x16` perceptual cosine deduplication."""

    def __init__(
        self,
        *,
        block_diff_threshold: float = BLOCK_DIFF_THRESHOLD,
        min_changed_blocks: int = MIN_CHANGED_BLOCKS,
        max_unstable_blocks: int = MAX_UNSTABLE_BLOCKS,
        stable_frames_required: int = STABLE_FRAMES_REQUIRED,
        cooldown_ms: float = COOLDOWN_MS,
        dedup_cosine_threshold: float = DEDUP_COSINE_THRESHOLD,
        max_slides: int = MAX_SLIDES,
        capture_initial: bool = False,
    ) -> None:
        self.block_diff_threshold = block_diff_threshold
        self.min_changed_blocks = min_changed_blocks
        self.max_unstable_blocks = max_unstable_blocks
        self.stable_frames_required = stable_frames_required
        self.cooldown_ms = cooldown_ms
        self.dedup_cosine_threshold = dedup_cosine_threshold
        self.max_slides = max_slides
        self.capture_initial = capture_initial

        self.captures: list[SlideCapture] = []
        self._accepted_blurred: np.ndarray | None = None
        self._accepted_vectors: list[np.ndarray] = []
        self._prev_blurred: np.ndarray | None = None
        self._pending_change: bool = False
        self._stable_count: int = 0
        self._last_capture_ms: float | None = None

    def _is_duplicate(self, vec: np.ndarray) -> bool:
        return any(
            cosine_similarity(vec, existing) >= self.dedup_cosine_threshold
            for existing in self._accepted_vectors
        )

    def _record_capture(
        self, frame: Any, blurred: np.ndarray, vec: np.ndarray, offset_ms: float
    ) -> SlideCapture:
        idx = len(self.captures) + 1
        ms_int = max(0, round(offset_ms))
        rel = f"img/slide-{idx:04d}-{ms_int}.png"
        cap = SlideCapture(index=idx, offset_ms=ms_int, file=rel, frame=frame)
        self.captures.append(cap)
        self._accepted_blurred = blurred
        self._accepted_vectors.append(vec)
        self._last_capture_ms = float(offset_ms)
        self._pending_change = False
        self._stable_count = 0
        return cap

    def feed(self, frame: Any, offset_ms: float) -> SlideCapture | None:
        """Process one ~1 fps video frame at `offset_ms`. Returns a `SlideCapture` when a new
        distinct slide has settled, or `None` otherwise."""
        if len(self.captures) >= self.max_slides:
            return None

        blurred = to_gray_blurred_160x90(frame)
        vec = perceptual_vector_16x16(blurred)

        # First frame establishes the baseline slide (and optionally captures it).
        if self._accepted_blurred is None:
            self._accepted_blurred = blurred
            self._accepted_vectors.append(vec)
            self._prev_blurred = blurred
            if self.capture_initial:
                idx = len(self.captures) + 1
                ms_int = max(0, round(offset_ms))
                rel = f"img/slide-{idx:04d}-{ms_int}.png"
                cap = SlideCapture(index=idx, offset_ms=ms_int, file=rel, frame=frame)
                self.captures.append(cap)
                self._last_capture_ms = float(offset_ms)
                return cap
            return None

        changed_vs_accepted = block_diff_count(
            blurred, self._accepted_blurred, self.block_diff_threshold
        )
        changed_vs_prev = (
            block_diff_count(blurred, self._prev_blurred, self.block_diff_threshold)
            if self._prev_blurred is not None
            else 0
        )
        self._prev_blurred = blurred

        # If screen returned to (or stayed within < 5 blocks of) the last accepted slide,
        # cancel any pending transition.
        if changed_vs_accepted < self.min_changed_blocks:
            self._pending_change = False
            self._stable_count = 0
            return None

        # >= 5 blocks differ from the last accepted slide.
        if not self._pending_change:
            self._pending_change = True
            self._stable_count = 0
            return None

        # Candidate transition is active: check frame-to-frame stability (< 2 blocks changing).
        if changed_vs_prev < self.max_unstable_blocks:
            self._stable_count += 1
        else:
            self._stable_count = 0
            return None

        if self._stable_count < self.stable_frames_required:
            return None

        # Enforce minimum cooldown between captures.
        if (
            self._last_capture_ms is not None
            and (float(offset_ms) - self._last_capture_ms) < self.cooldown_ms
        ):
            return None

        # Perceptual deduplication against all previously accepted slides (including slide 1).
        if self._is_duplicate(vec):
            self._accepted_blurred = blurred
            self._pending_change = False
            self._stable_count = 0
            return None

        return self._record_capture(frame, blurred, vec, offset_ms)


def extract_slides_from_frames(
    frames: Iterable[tuple[Any, float]],
    meeting_dir: Path | None = None,
    *,
    capture_initial: bool = False,
) -> list[dict[str, Any]]:
    """Run `SlideDetector` over `(frame, offset_ms)` pairs and optionally save PNGs to `meeting_dir`."""
    detector = SlideDetector(capture_initial=capture_initial)
    out: list[dict[str, Any]] = []
    for frame, offset_ms in frames:
        cap = detector.feed(frame, offset_ms)
        if cap is not None:
            if meeting_dir is not None:
                save_slide_png(cap.frame, meeting_dir / cap.file)
            out.append({"file": cap.file, "offset_ms": cap.offset_ms})
    return out


# Audio pass -----------------------------------------------------------------------------


def _is_16k_mono_pcm_wav(path: Path) -> bool:
    if path.suffix.lower() != ".wav":
        return False
    try:
        with wave.open(str(path), "rb") as w:
            return (
                w.getsampwidth() == 2
                and w.getnchannels() == 1
                and w.getframerate() == SAMPLE_RATE
                and w.getcomptype() == "NONE"
            )
    except (OSError, wave.Error):
        return False


def _write_wav_segment(path: Path, pcm_bytes: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as dst:
        dst.setnchannels(1)
        dst.setsampwidth(2)
        dst.setframerate(SAMPLE_RATE)
        dst.writeframes(pcm_bytes)


def _split_pcm_wav_fast(
    input_path: Path,
    meeting_dir: Path,
    on_progress: Callable[[float, str], None],
) -> tuple[list[dict[str, Any]], int]:
    """Pure-Python `wave` fast path when `input_path` is already a 16 kHz mono 16-bit PCM WAV."""
    out_dir = meeting_dir / "tracks" / TRACK_NAME
    out_dir.mkdir(parents=True, exist_ok=True)
    segments: list[dict[str, Any]] = []
    total_samples = 0

    with wave.open(str(input_path), "rb") as src:
        expected_total = max(1, src.getnframes())
        index = 0
        while True:
            frames = src.readframes(SEGMENT_SAMPLES)
            if not frames:
                break
            samples = len(frames) // 2
            name = f"{index:06d}.wav"
            rel = f"tracks/{TRACK_NAME}/{name}"
            _write_wav_segment(out_dir / name, frames)
            offset_ms = round(total_samples * 1000.0 / SAMPLE_RATE)
            duration_ms = round(samples * 1000.0 / SAMPLE_RATE)
            segments.append(
                {
                    "track": TRACK_NAME,
                    "segment": index,
                    "file": rel,
                    "offset_ms": offset_ms,
                    "duration_ms": duration_ms,
                    "samples": samples,
                }
            )
            total_samples += samples
            index += 1
            on_progress(min(1.0, total_samples / expected_total), f"audio segment {index}")

    total_duration_ms = round(total_samples * 1000.0 / SAMPLE_RATE)
    return segments, total_duration_ms


def _decode_audio_pyav(
    input_path: Path,
    meeting_dir: Path,
    on_progress: Callable[[float, str], None],
) -> tuple[list[dict[str, Any]], int]:
    """Decode any audio/video container to 60 s 16 kHz mono 16-bit WAV segments via PyAV."""
    import av

    out_dir = meeting_dir / "tracks" / TRACK_NAME
    out_dir.mkdir(parents=True, exist_ok=True)
    segments: list[dict[str, Any]] = []
    total_samples = 0
    index = 0
    chunk_bytes = SEGMENT_SAMPLES * 2
    buffer = bytearray()

    def flush_chunk(pcm_bytes: bytes) -> None:
        nonlocal total_samples, index
        samples = len(pcm_bytes) // 2
        if samples <= 0:
            return
        name = f"{index:06d}.wav"
        rel = f"tracks/{TRACK_NAME}/{name}"
        _write_wav_segment(out_dir / name, pcm_bytes)
        offset_ms = round(total_samples * 1000.0 / SAMPLE_RATE)
        duration_ms = round(samples * 1000.0 / SAMPLE_RATE)
        segments.append(
            {
                "track": TRACK_NAME,
                "segment": index,
                "file": rel,
                "offset_ms": offset_ms,
                "duration_ms": duration_ms,
                "samples": samples,
            }
        )
        total_samples += samples
        index += 1

    with av.open(str(input_path)) as container:
        if not container.streams.audio:
            raise ValueError(f"{input_path.name}: no audio stream found")
        audio_stream = container.streams.audio[0]
        duration_s = (
            float(container.duration) / 1_000_000.0
            if container.duration
            else (
                float(audio_stream.duration * audio_stream.time_base)
                if audio_stream.duration and audio_stream.time_base
                else 0.0
            )
        )
        expected_samples = max(1, int(duration_s * SAMPLE_RATE)) if duration_s > 0 else 0
        resampler = av.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)

        def consume_resampled(rframes: Iterable[Any]) -> None:
            for rf in rframes:
                arr = rf.to_ndarray().reshape(-1).astype("<i2", copy=False)
                buffer.extend(arr.tobytes())
                while len(buffer) >= chunk_bytes:
                    piece = bytes(buffer[:chunk_bytes])
                    del buffer[:chunk_bytes]
                    flush_chunk(piece)
                    if expected_samples > 0:
                        frac = min(1.0, total_samples / expected_samples)
                    else:
                        frac = min(0.95, index / (index + 2))
                    on_progress(frac, f"audio segment {index}")

        for frame in container.decode(audio=0):
            consume_resampled(resampler.resample(frame))
        consume_resampled(resampler.resample(None))

    if buffer:
        flush_chunk(bytes(buffer))
        buffer.clear()
        on_progress(1.0, f"audio segment {index}")

    total_duration_ms = round(total_samples * 1000.0 / SAMPLE_RATE)
    return segments, total_duration_ms


def decode_audio_segments(
    input_path: Path,
    meeting_dir: Path,
    on_progress: Callable[[float, str], None] = lambda _f, _m: None,
) -> tuple[list[dict[str, Any]], int]:
    """Decode `input_path` into 60 s 16 kHz mono WAV segments under `tracks/imported/`."""
    if _is_16k_mono_pcm_wav(input_path):
        return _split_pcm_wav_fast(input_path, meeting_dir, on_progress)
    return _decode_audio_pyav(input_path, meeting_dir, on_progress)


def _extract_video_slides_pyav(
    input_path: Path,
    meeting_dir: Path,
    duration_ms: int,
    on_progress: Callable[[float, str], None],
) -> list[dict[str, Any]]:
    """Sample video stream at ~1 fps and extract stable slides via `SlideDetector`."""
    import av

    try:
        container = av.open(str(input_path))
    except Exception:  # noqa: BLE001
        return []

    with container:
        if not container.streams.video:
            return []
        video_stream = container.streams.video[0]
        rate = float(video_stream.average_rate or 25.0)
        detector = SlideDetector(capture_initial=True)
        screenshots: list[dict[str, Any]] = []
        last_sample_ms: float | None = None

        for frame_idx, frame in enumerate(container.decode(video=0)):
            if frame.time is not None:
                offset_ms = float(frame.time) * 1000.0
            elif frame.pts is not None and video_stream.time_base is not None:
                offset_ms = float(frame.pts * video_stream.time_base) * 1000.0
            else:
                offset_ms = frame_idx * (1000.0 / max(1.0, rate))

            if last_sample_ms is not None and (offset_ms - last_sample_ms) < 950.0:
                continue
            last_sample_ms = offset_ms

            cap = detector.feed(frame, offset_ms)
            if cap is not None:
                save_slide_png(cap.frame, meeting_dir / cap.file)
                screenshots.append({"file": cap.file, "offset_ms": cap.offset_ms})
            if duration_ms > 0:
                on_progress(
                    min(1.0, offset_ms / duration_ms),
                    f"scanning slides ({len(screenshots)} captured)",
                )
            if len(screenshots) >= MAX_SLIDES:
                break

    return screenshots


def ingest_media(
    input_path: Path,
    meeting_dir: Path,
    *,
    mode: str = "audio_and_slides",
    meeting_id: str = "imported",
) -> dict[str, Any]:
    """Ingest `input_path` into `meeting_dir`, writing `work/ingest.json` and emitting progress."""
    input_path = Path(input_path)
    meeting_dir = Path(meeting_dir)
    if not input_path.exists():
        raise FileNotFoundError(f"input media file not found: {input_path}")
    if mode not in ("audio_only", "audio_and_slides"):
        raise ValueError(f"unsupported ingest mode: {mode}")

    protocol.progress(STEP, 0.05, f"decoding audio from {input_path.name}")
    want_slides = mode == "audio_and_slides" and input_path.suffix.lower() not in (
        ".wav",
        ".mp3",
        ".flac",
        ".ogg",
        ".aac",
        ".m4a",
    )
    audio_span = 0.45 if want_slides else 0.90

    def audio_progress(frac: float, msg: str) -> None:
        protocol.progress(STEP, 0.05 + audio_span * min(1.0, max(0.0, frac)), msg)

    segments, duration_ms = decode_audio_segments(input_path, meeting_dir, audio_progress)
    protocol.progress(STEP, 0.05 + audio_span, f"{len(segments)} audio segments")

    screenshots: list[dict[str, Any]] = []
    if want_slides:
        protocol.progress(STEP, 0.52, "extracting slides")

        def slide_progress(frac: float, msg: str) -> None:
            protocol.progress(STEP, 0.52 + 0.43 * min(1.0, max(0.0, frac)), msg)

        screenshots = _extract_video_slides_pyav(
            input_path, meeting_dir, duration_ms, slide_progress
        )
        protocol.progress(STEP, 0.95, f"{len(screenshots)} slides extracted")

    tracks = [
        {
            "name": TRACK_NAME,
            "source": f"file:{input_path.name}",
            "offset_ms": 0,
            "segments": [s["file"] for s in segments],
        }
    ]
    doc = {
        "version": 1,
        "meeting_id": meeting_id,
        "duration_ms": duration_ms,
        "tracks": tracks,
        "segments": segments,
        "screenshots": screenshots,
    }
    work_dir = meeting_dir / "work"
    work_dir.mkdir(parents=True, exist_ok=True)
    write_json(work_dir / "ingest.json", doc)
    protocol.progress(
        STEP, 1.0, f"{len(segments)} segments, {len(screenshots)} slides, {duration_ms} ms"
    )
    return doc


def run(manifest: Manifest | dict[str, Any], meeting_dir: Path | None = None) -> dict[str, Any]:
    """Entry point for `ingest` step when invoked via manifest or dict."""
    if isinstance(manifest, Manifest):
        m_dir = meeting_dir or manifest.meeting_dir
        m_id = manifest.meeting_id
        options = manifest.options
    else:
        m_dir = Path(meeting_dir or manifest.get("meeting_dir", "."))
        m_id = str(manifest.get("meeting_id", "imported"))
        options = manifest.get("options", manifest)

    ingest_opts = options.get("ingest") if isinstance(options.get("ingest"), dict) else options
    input_raw = ingest_opts.get("input_path") or ingest_opts.get("input")
    if not input_raw:
        raise ValueError("ingest requires options.ingest.input_path")
    mode = str(ingest_opts.get("mode", "audio_and_slides"))
    return ingest_media(Path(input_raw), m_dir, mode=mode, meeting_id=m_id)


def load_ingest_manifest(manifest_path: Path, meeting_dir: Path) -> dict[str, Any]:
    """Load a manifest JSON for `ingest` without rejecting `options.ingest`."""
    doc = json.loads(manifest_path.read_text(encoding="utf-8"))
    doc.setdefault("meeting_dir", str(meeting_dir))
    return doc
