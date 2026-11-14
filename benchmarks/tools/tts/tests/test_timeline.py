import numpy as np

from bbb_tts import timeline
from bbb_tts.rttm import to_rttm

RATE = 16000
TURNS = [
    {"speaker": "a", "lang": "pl", "text": "x"},
    {"speaker": "b", "lang": "en", "text": "y"},
    {"speaker": "a", "lang": "pl", "text": "z"},
]


def test_pauses_deterministic_and_in_range():
    p1 = timeline.plan_pauses(50, seed=3, sample_rate=RATE)
    p2 = timeline.plan_pauses(50, seed=3, sample_rate=RATE)
    p3 = timeline.plan_pauses(50, seed=4, sample_rate=RATE)
    assert p1 == p2 and p1 != p3
    assert len(p1) == 49
    assert all(0.2 * RATE <= p <= 0.6 * RATE for p in p1)


def test_pauses_edge_sizes():
    assert timeline.plan_pauses(0, 1, RATE) == []
    assert timeline.plan_pauses(1, 1, RATE) == []


def test_layout_places_turns_sequentially():
    segs = timeline.layout(TURNS, [1000, 2000, 500], [100, 200])
    assert [(s.start, s.end) for s in segs] == [(0, 1000), (1100, 3100), (3300, 3800)]
    assert [s.speaker for s in segs] == ["a", "b", "a"]


def test_layout_lead_in_and_validation():
    segs = timeline.layout(TURNS, [10, 10, 10], [5, 5], lead_in=7)
    assert segs[0].start == 7
    try:
        timeline.layout(TURNS, [10, 10, 10], [5])
    except ValueError:
        pass
    else:
        raise AssertionError("wrong pause count must be rejected")


def test_to_timeline_rounds_to_milliseconds():
    segs = timeline.layout(TURNS, [RATE, RATE // 3, RATE], [RATE // 4, RATE // 4])
    tl = timeline.to_timeline(segs, RATE)
    assert tl[0] == {"turn": 0, "speaker": "a", "lang": "pl", "start_s": 0.0, "end_s": 1.0}
    assert tl[1]["start_s"] == 1.25
    assert tl[1]["end_s"] == round((RATE + RATE // 4 + RATE // 3) / RATE, 3)


def test_mix_and_tracks_share_one_axis():
    waves = [
        np.full(4, 0.5, np.float32),
        np.full(2, -0.25, np.float32),
        np.full(3, 0.1, np.float32),
    ]
    segs = timeline.layout(TURNS, [4, 2, 3], [1, 2])
    mixed = timeline.mix(segs, waves)
    assert len(mixed) == 4 + 1 + 2 + 2 + 3
    assert list(mixed[:4]) == [0.5] * 4 and mixed[4] == 0
    tracks = timeline.speaker_tracks(segs, waves, len(mixed))
    assert set(tracks) == {"a", "b"}
    assert all(len(t) == len(mixed) for t in tracks.values())
    # Tracks add up to the mix because turns never overlap.
    assert np.allclose(sum(tracks.values()), mixed)
    assert np.count_nonzero(tracks["b"]) == 2


def test_rttm_format():
    segs = timeline.layout(TURNS, [RATE, RATE, RATE], [RATE // 2, RATE // 2])
    text = to_rttm(timeline.to_timeline(segs, RATE), "mini-pl")
    lines = text.strip().splitlines()
    assert len(lines) == 3
    assert lines[0] == "SPEAKER mini-pl 1 0.000 1.000 <NA> <NA> a <NA> <NA>"
    assert lines[1] == "SPEAKER mini-pl 1 1.500 1.000 <NA> <NA> b <NA> <NA>"
    assert to_rttm([], "x") == ""
