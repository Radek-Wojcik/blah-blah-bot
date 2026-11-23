"""Unit tests for granular progress monotonicity across worker steps, key-slide selection,
and automatic `short_title` derivation."""

from __future__ import annotations

import itertools
import sys
import types
import wave
from pathlib import Path
from typing import Any

import numpy as np

from bbb_worker import llama, manifest, models, protocol, schema
from bbb_worker.manifest import SAMPLE_RATE
from bbb_worker.steps import asr, diarize, extract, read_json, reduce, vision, write_json

from .llm_support import (
    FakeLlm,
    by_kind,
    context_doc,
    extraction_for,
    llm_options,
    long_transcript,
    make_meeting,
)


def _assert_strictly_monotonic_intermediate(fractions: list[float]) -> None:
    """Prove `0.0 < p1 <= p2 < 1.0` for all intermediate progress events, ending at `1.0`."""
    assert len(fractions) >= 3, f"expected granular progress events, got {fractions}"
    assert fractions[-1] == 1.0
    intermediate = fractions[:-1]
    assert all(0.0 < p < 1.0 for p in intermediate), intermediate
    assert all(0.0 < p1 <= p2 < 1.0 for p1, p2 in itertools.pairwise(intermediate)), intermediate


def _write_pcm_wav(path: Path, seconds: float = 2.0) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    n = round(seconds * SAMPLE_RATE)
    pcm = (np.sin(np.linspace(0.0, 40.0, n)) * 8000).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm.tobytes())


def test_asr_emits_granular_monotonic_progress(tmp_path: Path, monkeypatch):
    seg0 = tmp_path / "mic" / "000000.wav"
    _write_pcm_wav(seg0, seconds=4.0)
    m = manifest.Manifest(
        meeting_id="m-asr",
        meeting_dir=tmp_path,
        tracks=(manifest.Track("mic", "microphone", 0.0, ("mic/000000.wav",)),),
        screenshots=(),
        options={"language": "en", "asr_model": "turbo"},
    )

    class FakeWord:
        def __init__(self, start: float, end: float, word: str) -> None:
            self.start = start
            self.end = end
            self.word = word

    class FakeSeg:
        def __init__(self, start: float, end: float, text: str) -> None:
            self.start = start
            self.end = end
            self.text = text
            self.words = [FakeWord(start, end, text)]

    class FakeInfo:
        language = "en"

    class FakeWhisperModel:
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            pass

        def transcribe(self, audio: Any, **kwargs: Any):
            _ = (audio, kwargs)
            return [FakeSeg(0.0, 1.5, "Hello"), FakeSeg(1.5, 3.5, "world")], FakeInfo()

    fake_fw = types.ModuleType("faster_whisper")
    fake_fw.WhisperModel = FakeWhisperModel  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "faster_whisper", fake_fw)
    monkeypatch.setattr(models, "local_path", lambda _key: tmp_path)

    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": fractions.append(fraction) if step == "asr" else None,
    )

    asr.run(m)
    assert fractions[0] == 0.05
    assert fractions[1] == 0.10
    _assert_strictly_monotonic_intermediate(fractions)


def test_diarize_hook_maps_stages_and_throttles_to_200ms(monkeypatch):
    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": (
            fractions.append(fraction) if step == "diarize" else None
        ),
    )

    now = [0.0]
    hook = diarize.make_diarize_hook(
        "meet", track_index=0, total_tracks=1, throttle_s=0.2, clock=lambda: now[0]
    )

    # Segmentation: 0.05..0.45
    hook("segmentation", None, completed=1, total=4)
    assert len(fractions) == 1 and 0.05 <= fractions[-1] <= 0.45

    # Rapid intra-step update 50 ms later (< 200 ms) is throttled.
    now[0] = 0.05
    hook("segmentation", None, completed=2, total=4)
    assert len(fractions) == 1

    # Update at 250 ms (>= 200 ms) is emitted.
    now[0] = 0.25
    hook("segmentation", None, completed=3, total=4)
    assert len(fractions) == 2 and fractions[-1] > fractions[0]

    # Step completion is emitted even within 200 ms.
    now[0] = 0.30
    hook("segmentation", None, completed=4, total=4)
    assert fractions[-1] == 0.45

    # Embeddings: 0.45..0.90
    now[0] = 0.35
    hook("embeddings", None, completed=1, total=2)
    assert 0.45 < fractions[-1] < 0.90
    now[0] = 0.40
    hook("embeddings", None, completed=2, total=2)
    assert fractions[-1] == 0.90

    # Clustering / speaker_counting: 0.90..0.98
    now[0] = 0.45
    hook("speaker_counting", None, completed=1, total=2)
    assert 0.90 <= fractions[-1] <= 0.98
    now[0] = 0.50
    hook("clustering", None, completed=2, total=2)
    assert fractions[-1] == 0.98

    assert all(0.0 < p1 <= p2 < 1.0 for p1, p2 in itertools.pairwise(fractions))


def test_diarize_falls_back_gracefully_when_mock_pipeline_rejects_hook(tmp_path: Path, monkeypatch):
    seg0 = tmp_path / "meet" / "000000.wav"
    _write_pcm_wav(seg0, seconds=2.0)
    m = manifest.Manifest(
        meeting_id="m-diar",
        meeting_dir=tmp_path,
        tracks=(manifest.Track("meet", "app:meet.exe", 0.0, ("meet/000000.wav",)),),
        screenshots=(),
        options={"language": "en"},
    )

    class FakeAnnotation:
        def itertracks(self, yield_label: bool = False):
            _ = yield_label
            return []

        def labels(self):
            return ["SPEAKER_00"]

    class FakeOut:
        exclusive_speaker_diarization = FakeAnnotation()
        speaker_diarization = FakeAnnotation()
        speaker_embeddings = np.ones((1, 4), dtype=np.float32)

    class MockPipelineNoHook:
        @classmethod
        def from_pretrained(cls, _path: str):
            return cls()

        def to(self, _device: Any):
            return self

        def __call__(self, audio_input: dict[str, Any]):
            # Intentionally does NOT accept `hook=` keyword argument.
            _ = audio_input
            return FakeOut()

    fake_torch = types.ModuleType("torch")
    fake_torch.device = lambda name: name  # type: ignore[attr-defined]
    fake_torch.from_numpy = lambda arr: arr  # type: ignore[attr-defined]
    fake_pyannote = types.ModuleType("pyannote.audio")
    fake_pyannote.Pipeline = MockPipelineNoHook  # type: ignore[attr-defined]

    monkeypatch.setitem(sys.modules, "torch", fake_torch)
    monkeypatch.setitem(sys.modules, "pyannote", types.ModuleType("pyannote"))
    monkeypatch.setitem(sys.modules, "pyannote.audio", fake_pyannote)
    monkeypatch.setattr(models, "local_path", lambda _key: tmp_path)

    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": (
            fractions.append(fraction) if step == "diarize" else None
        ),
    )

    diarize.run(m)
    assert fractions[0] == 0.05
    _assert_strictly_monotonic_intermediate(fractions)
    out = read_json(m.work_dir / "diarize" / "meet.json")
    assert out["speakers"] == ["SPEAKER_00"]


def test_vision_emits_granular_monotonic_progress(tmp_path: Path, monkeypatch):
    work = tmp_path / "work"
    work.mkdir(parents=True, exist_ok=True)
    write_json(
        work / "transcript.json",
        {
            "version": 1,
            "meeting_id": "m-vis",
            "language": "en",
            "speakers": [{"id": "user", "label": "You", "kind": "user"}],
            "segments": [],
        },
    )
    for idx in (1, 2, 3):
        img = tmp_path / "screenshots" / f"s{idx}.png"
        img.parent.mkdir(parents=True, exist_ok=True)
        img.write_bytes(b"fake-png")

    m = manifest.Manifest(
        meeting_id="m-vis",
        meeting_dir=tmp_path,
        tracks=(),
        screenshots=(
            ("screenshots/s1.png", 1000.0),
            ("screenshots/s2.png", 5000.0),
            ("screenshots/s3.png", 9000.0),
        ),
        options={"vision_base_url": "http://127.0.0.1:9/v1"},
    )
    monkeypatch.setattr(
        llama,
        "chat",
        lambda _base, _payload: {
            "choices": [{"message": {"content": "Slide chart with Q4 numbers."}}]
        },
    )

    fractions: list[float] = []
    monkeypatch.setattr(
        protocol,
        "progress",
        lambda step, fraction, message="": fractions.append(fraction) if step == "vision" else None,
    )

    vision.run(m)
    assert fractions[0] == 0.05
    # 0.05 start + 2 events (before & after) per 3 screenshots + 1.0 finish = 8 events
    assert len(fractions) == 8
    _assert_strictly_monotonic_intermediate(fractions)


def test_extract_and_reduce_emit_granular_monotonic_progress_and_derive_short_title_and_key_slides(
    tmp_path: Path, monkeypatch
):
    special = {
        20: "We decided to adopt the new lithium battery architecture for Q4.",
        80: "Alex will finalize the supplier contract by Friday.",
    }
    rules = [
        {
            "quote": special[20],
            "text": "Adopt the new lithium battery architecture for Q4",
            "fields": {"type": "decision"},
        },
        {
            "quote": special[80],
            "text": "Alex finalizes the supplier contract",
            "fields": {"type": "task", "assignee": "alex"},
        },
    ]
    descriptions = {
        "version": 1,
        "meeting_id": "m1",
        "items": [
            {
                "file": "img/blank.png",
                "offset_ms": 1000,
                "description": "Blank screen.",
                "model": "v",
            },
            {
                "file": "img/slide-1.png",
                "offset_ms": 5000,
                "description": "Architecture diagram of the Q4 lithium battery pack showing 48V modules and 120A BMS limits.",
                "model": "v",
            },
            {
                "file": "img/slide-1-dup.png",
                "offset_ms": 11000,
                "description": "Architecture diagram of the Q4 lithium battery pack showing 48V modules and 120A BMS limits.",
                "model": "v",
            },
            {
                "file": "img/slide-2.png",
                "offset_ms": 25000,
                "description": "Budget comparison table with supplier quotes: Supplier A 2400 EUR, Supplier B 1950 EUR, delivery schedule.",
                "model": "v",
            },
        ],
    }

    extract_fractions: list[float] = []
    reduce_fractions: list[float] = []

    def record_progress(step: str, fraction: float, message: str = "") -> None:
        _ = message
        if step == "extract":
            extract_fractions.append(fraction)
        elif step == "reduce":
            reduce_fractions.append(fraction)

    monkeypatch.setattr(protocol, "progress", record_progress)

    h = by_kind(
        extraction=extraction_for(rules),
        summary=lambda _b, _n: {
            "summary": "The engineering team approved the Q4 lithium battery architecture and reviewed supplier contracts."
        },
        sections=lambda _b, _n: {
            "sections": [{"id": "battery", "title": "Battery", "description": "Architecture"}],
            "assignments": [
                {"item": "i001", "section": "battery"},
                {"item": "i002", "section": "battery"},
            ],
        },
    )
    with FakeLlm(h) as fake:
        m = make_meeting(
            tmp_path,
            long_transcript(25, special),
            context_doc(mode="auto"),
            llm_options(fake.url),
            files={"descriptions.json": descriptions},
        )
        extract.run(m)
        reduce.run(m)

    assert extract_fractions[0] == 0.05
    _assert_strictly_monotonic_intermediate(extract_fractions)

    assert reduce_fractions[0] == 0.05
    _assert_strictly_monotonic_intermediate(reduce_fractions)

    draft_doc = read_json(m.work_dir / "draft.json")
    schema.validate("meeting", draft_doc)
    assert "short_title" in draft_doc
    words = draft_doc["short_title"].split()
    assert 3 <= len(words) <= 8
    assert len(draft_doc["short_title"]) <= 70
    assert draft_doc["key_screenshots"] == ["img/slide-1.png", "img/slide-2.png"]


def test_derive_short_title_and_select_key_screenshots_edge_cases():
    # Fallback to decision/topic when summary is short
    items = [
        {"type": "note", "text": "Short"},
        {
            "type": "decision",
            "text": "Migrate the production database to PostgreSQL cluster next Monday",
        },
    ]
    title = reduce.derive_short_title("Ok.", items)
    assert 3 <= len(title.split()) <= 8
    assert len(title) <= 70
    assert "Migrate the production database" in title

    # Cap at max_screenshots (default 8)
    many = [
        {
            "file": f"img/s{i}.png",
            "offset_ms": i * 6000,
            "description": f"Detailed technical chart number {i} with unique metric {i * 137} and vocabulary_{i}_alpha_{i * 19}.",
        }
        for i in range(15)
    ]
    selected = reduce.select_key_screenshots(many)
    assert len(selected) == 8
