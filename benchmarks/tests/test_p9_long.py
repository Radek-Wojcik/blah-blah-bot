import json
import wave

import pytest

from bbb_bench.proto import p9_long


def _wav(path, seconds):
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(b"\x01\x00" * int(16000 * seconds))


def test_build_repeats_sources_until_target_length(tmp_path):
    _wav(tmp_path / "audio" / "a" / "mixed.wav", 2)
    _wav(tmp_path / "audio" / "b" / "mixed.wav", 3)
    out = p9_long.build(tmp_path / "audio", tmp_path / "long", hours=20 / 3600)
    assert p9_long._seconds(out) == pytest.approx(20.0)


def test_estimate_adds_stages(tmp_path):
    (tmp_path / "llm.json").write_text(
        json.dumps(
            {
                "config": {"model": "m"},
                "seconds_per_1000_words": 20.0,
                "peak_vram_above_baseline_mib": 9000,
            }
        )
    )
    (tmp_path / "asr.json").write_text(
        json.dumps(
            {
                "audio_s": 36000,
                "words": 50000,
                "processing_s": 1000,
                "model_load_s": 10,
                "peak_vram_above_baseline_mib": 300,
            }
        )
    )
    (tmp_path / "diar.json").write_text(
        json.dumps({"processing_s": 600, "model_load_s": 20, "peak_vram_above_baseline_mib": 10000})
    )
    est = p9_long.estimate(tmp_path / "llm.json", tmp_path / "asr.json", tmp_path / "diar.json")
    assert est["stages"]["extraction_s"] == pytest.approx(1000.0)
    assert est["total_s"] == pytest.approx(1010 + 620 + 1000)
    assert est["peak_vram_above_baseline_mib"] == 10000
