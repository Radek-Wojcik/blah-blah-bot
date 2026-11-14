import pytest

from bbb_bench.metrics import der


def test_perfect_with_renamed_speakers():
    ref = [(0, 10, "A"), (10, 20, "B")]
    hyp = [(0, 10, "x"), (10, 20, "y")]
    assert der(ref, hyp)["der"] == 0.0


def test_missed_speech():
    # 20 s of reference speech, hypothesis covers only the first 15 s => miss 5/20
    ref = [(0, 10, "A"), (10, 20, "B")]
    hyp = [(0, 10, "x"), (10, 15, "y")]
    r = der(ref, hyp)
    assert r["der"] == pytest.approx(0.25)
    assert r["missed"] == pytest.approx(0.25)


def test_false_alarm():
    # 10 s of reference speech; hypothesis has 5 extra seconds => FA 5/10
    ref = [(0, 10, "A")]
    hyp = [(0, 15, "x")]
    r = der(ref, hyp)
    assert r["der"] == pytest.approx(0.5)
    assert r["false_alarm"] == pytest.approx(0.5)


def test_confusion():
    # Reference: A 0-10, B 10-20. Hypothesis: x 0-15, y 15-20; mapping x->A, y->B.
    # 10-15 is B in the reference but x (= A) in the hypothesis => 5 s of 20
    ref = [(0, 10, "A"), (10, 20, "B")]
    hyp = [(0, 15, "x"), (15, 20, "y")]
    r = der(ref, hyp)
    assert r["der"] == pytest.approx(0.25)
    assert r["confusion"] == pytest.approx(0.25)


def test_overlap_counts_per_speaker():
    # Reference: A 0-10, B 5-10 => 15 speaker-seconds. Hypothesis: x 0-10 only.
    # During 5-10 two reference speakers, one hypothesis speaker => 5 s missed.
    ref = [(0, 10, "A"), (5, 10, "B")]
    hyp = [(0, 10, "x")]
    r = der(ref, hyp)
    assert r["der"] == pytest.approx(5 / 15)
    assert r["total_speech_s"] == pytest.approx(15)


def test_empty_reference_raises():
    with pytest.raises(ValueError):
        der([], [(0, 1, "x")])
