import numpy as np
import pytest

from bbb_bench.proto import diar_common as dc


def test_read_rttm(tmp_path):
    f = tmp_path / "x.rttm"
    f.write_text(
        "SPEAKER m 1 0.500 2.000 <NA> <NA> alex <NA> <NA>\n"
        "SPEAKER m 1 3.000 1.500 <NA> <NA> kasia <NA> <NA>\n",
        encoding="utf-8",
    )
    assert dc.read_rttm(f) == [(0.5, 2.5, "alex"), (3.0, 4.5, "kasia")]


def test_best_overlap_mapping():
    ref = [(0, 10, "alex"), (10, 20, "kasia")]
    hyp = [(0, 8, "S0"), (8, 12, "S1"), (12, 20, "S1")]
    mapping = dc.best_overlap_mapping(ref, hyp)
    assert mapping["S0"] == ("alex", 8)
    assert mapping["S1"][0] == "kasia"


def test_cosine():
    assert dc.cosine(np.array([1.0, 0.0]), np.array([1.0, 0.0])) == pytest.approx(1.0)
    assert dc.cosine(np.array([1.0, 0.0]), np.array([0.0, 1.0])) == pytest.approx(0.0)
    assert dc.cosine(np.zeros(2), np.array([1.0, 0.0])) == 0.0


def test_read_wav_rejects_stereo(tmp_path):
    import wave

    f = tmp_path / "s.wav"
    with wave.open(str(f), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(b"\x00" * 400)
    with pytest.raises(ValueError):
        dc.read_wav_mono16k(f)


def test_voiced_reference_trims_silence_and_bridges_short_gaps():
    rate = 16000
    t = np.arange(rate * 10) / rate
    tone = 0.3 * np.sin(2 * np.pi * 200 * t)
    audio = np.zeros(rate * 10)
    audio[rate * 1 : rate * 4] = tone[rate * 1 : rate * 4]  # 1-4 s
    audio[rate * 5 : int(rate * 5.2)] = tone[rate * 5 : int(rate * 5.2)]  # 5.0-5.2 s
    audio[int(rate * 5.4) : rate * 7] = tone[int(rate * 5.4) : rate * 7]  # gap of 0.2 s bridged
    out = dc.voiced_reference(audio.astype(np.float32), rate, [(0.0, 9.0, "alex")])
    covered = sum(b - a for a, b, _ in out)
    assert covered == pytest.approx(3.0 + 2.0, abs=0.1)
    assert all(s == "alex" for _, _, s in out)
    assert dc.voiced_reference(np.zeros(100, dtype=np.float32), rate, [(0, 1, "x")]) == []
