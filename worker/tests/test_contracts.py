"""Contract tests: the schemas accept what the worker produces and reject what it must not."""

import copy
import json
import wave
from pathlib import Path

import jsonschema
import pytest

from bbb_worker import manifest, schema
from bbb_worker.steps import vision

FIXTURES = Path(__file__).parent / "fixtures"


def fixture(name):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def test_transcript_fixture_is_valid():
    schema.validate("transcript", fixture("transcript.json"))


def test_screenshot_descriptions_fixture_is_valid():
    schema.validate("screenshot-descriptions", fixture("descriptions.json"))


def test_manifest_fixture_is_valid():
    schema.validate("job-manifest", fixture("manifest.json"))


@pytest.mark.parametrize(
    "mutate",
    [
        lambda d: d.pop("speakers"),
        lambda d: d["segments"][0].update(start_ms=-1),
        lambda d: d["segments"][0].pop("words"),
        lambda d: d["segments"][0].update(unexpected=True),
        lambda d: d.update(version=2),
        lambda d: d["speakers"][0].update(kind="robot"),
    ],
)
def test_transcript_violations_are_rejected(mutate):
    doc = copy.deepcopy(fixture("transcript.json"))
    mutate(doc)
    with pytest.raises(jsonschema.ValidationError):
        schema.validate("transcript", doc)


def test_manifest_rejects_unknown_language():
    doc = fixture("manifest.json")
    doc["options"]["language"] = "de"
    with pytest.raises(jsonschema.ValidationError):
        schema.validate("job-manifest", doc)


def test_manifest_loads_and_reads_segments(tmp_path):
    doc = fixture("manifest.json")
    (tmp_path / "m.json").write_text(json.dumps(doc), encoding="utf-8")
    m = manifest.load(tmp_path / "m.json", tmp_path)
    assert m.tracks[0].is_microphone
    assert m.language == "auto"

    seg = tmp_path / doc["tracks"][0]["segments"][0]
    seg.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(seg), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(b"\x00\x00" * 1600)
    assert len(manifest.load_track_audio(m, m.tracks[0])) == 1600


def test_manifest_rejects_wrong_sample_rate(tmp_path):
    bad = tmp_path / "bad.wav"
    with wave.open(str(bad), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(44100)
        w.writeframes(b"\x00\x00" * 10)
    with pytest.raises(ValueError):
        manifest.read_wav(bad)


def test_vision_context_uses_speaker_labels_and_is_capped():
    doc = fixture("transcript.json")
    text = vision.context_text(doc, 1500)
    assert text.startswith("You: ")
    doc["segments"][0]["text"] = "x" * 10_000
    assert len(vision.context_text(doc, 1500)) <= vision.MAX_CONTEXT_CHARS


def test_vision_payload_embeds_the_image():
    payload = vision.build_payload(b"png", "describe")
    parts = payload["messages"][0]["content"]
    assert parts[1]["image_url"]["url"].startswith("data:image/png;base64,")
