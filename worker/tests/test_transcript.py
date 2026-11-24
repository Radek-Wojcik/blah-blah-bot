import random
import time

from bbb_worker import schema, transcript


def word(text, start, end):
    return {"start_ms": start, "end_ms": end, "text": text}


def asr(*words, lang="en"):
    return {"language": lang, "segments": [{"words": list(words)}]}


TRACKS = [
    {"name": "mic", "source": "microphone"},
    {"name": "discord", "source": "app:discord.exe"},
]


def test_microphone_is_the_user_without_diarization():
    doc = transcript.merge("m", TRACKS[:1], {"mic": asr(word(" hello", 0, 400))}, {})
    assert doc["speakers"] == [{"id": "user", "label": "You", "kind": "user"}]
    assert doc["segments"][0]["speaker"] == "user"
    schema.validate("transcript", doc)


def test_track_without_diarization_is_one_source_speaker():
    doc = transcript.merge("m", TRACKS[1:], {"discord": asr(word(" hi", 0, 300))}, {})
    assert doc["speakers"][0]["label"] == "Discord"
    assert doc["speakers"][0]["kind"] == "source"


def test_words_follow_the_speaker_turns():
    words = [word(" a", 0, 400), word(" b", 500, 900), word(" c", 2000, 2400)]
    turns = {"discord": [(0, 1000, "S0"), (1900, 3000, "S1")]}
    doc = transcript.merge("m", TRACKS[1:], {"discord": asr(*words)}, turns)
    assert [s["speaker"] for s in doc["segments"]] == ["discord:S0", "discord:S1"]
    assert [s["text"] for s in doc["segments"]] == ["a b", "c"]
    assert [s["label"] for s in doc["speakers"]] == ["Speaker 1", "Speaker 2"]


def test_word_overlapping_two_speakers_goes_to_the_larger_overlap():
    turns = [(0, 1000, "A"), (1000, 2000, "B")]
    assert transcript.speaker_for_word(800, 1600, turns) == "B"


def test_word_in_a_gap_is_attached_to_a_near_turn_or_unknown():
    turns = [(0, 1000, "A")]
    assert transcript.speaker_for_word(1100, 1300, turns) == "A"
    assert transcript.speaker_for_word(5000, 5200, turns) is None


def test_long_pause_splits_a_segment_of_the_same_speaker():
    words = [word(" a", 0, 400), word(" b", 400 + transcript.MAX_GAP_MS + 1, 3000)]
    doc = transcript.merge("m", TRACKS[:1], {"mic": asr(*words)}, {})
    assert len(doc["segments"]) == 2


def test_overlapping_tracks_are_interleaved_by_start_time():
    mic = asr(word(" one", 0, 500), word(" three", 3000, 3500))
    app = asr(word(" two", 1000, 1500))
    doc = transcript.merge("m", TRACKS, {"mic": mic, "discord": app}, {})
    assert [s["text"] for s in doc["segments"]] == ["one", "two", "three"]
    assert [s["id"] for s in doc["segments"]] == [0, 1, 2]


def test_overlapping_speech_is_kept_from_both_tracks():
    doc = transcript.merge(
        "m",
        TRACKS,
        {"mic": asr(word(" yes", 1000, 1600)), "discord": asr(word(" no", 1200, 1800))},
        {},
    )
    assert len(doc["segments"]) == 2


def test_dominant_language_is_by_speech_time():
    doc = transcript.merge(
        "m",
        TRACKS,
        {
            "mic": asr(word(" tak", 0, 500), lang="pl"),
            "discord": asr(word(" yes", 0, 3000), lang="en"),
        },
        {},
    )
    assert doc["language"] == "en"


def test_track_with_no_speech_gives_an_empty_transcript_that_validates():
    doc = transcript.merge("m", TRACKS[:1], {"mic": {"language": "und", "segments": []}}, {})
    assert doc["segments"] == []
    schema.validate("transcript", doc)


def test_context_window():
    doc = {
        "segments": [
            {"start_ms": 0, "end_ms": 1000},
            {"start_ms": 200_000, "end_ms": 201_000},
        ]
    }
    assert len(transcript.context_around(doc, 30_000)) == 1


def _linear_reference(start_ms: float, end_ms: float, turns: list[transcript.Turn]) -> str | None:
    best: tuple[float, str] | None = None
    for t_start, t_end, who in turns:
        overlap = min(end_ms, t_end) - max(start_ms, t_start)
        if overlap > 0 and (best is None or overlap > best[0]):
            best = (overlap, who)
    if best:
        return best[1]
    mid = (start_ms + end_ms) / 2
    nearest: tuple[float, str] | None = None
    for t_start, t_end, who in turns:
        distance = max(t_start - mid, mid - t_end, 0.0)
        if nearest is None or distance < nearest[0]:
            nearest = (distance, who)
    if nearest and nearest[0] <= transcript.NEAREST_TOLERANCE_MS:
        return nearest[1]
    return None


def test_turn_index_matches_linear_scan_on_overlapping_ties_and_unsorted_turns():
    assert transcript.speaker_for_word(0, 100, []) is None
    assert transcript.TurnIndex([]).speaker_for_word(0, 100) is None

    # Unsorted, nested, equal-overlap ties, equal-distance ties, zero-width words.
    turns: list[transcript.Turn] = [
        (2000, 3000, "B"),
        (0, 10000, "Long"),
        (1000, 2000, "A"),
        (1500, 2500, "C"),
        (12000, 13000, "D"),
        (14200, 15000, "E"),
    ]
    index = transcript.TurnIndex(turns)
    probes = [
        (0, 500),
        (1000, 2000),
        (1500, 2500),
        (1800, 2200),
        (5000, 5000),  # zero-length word inside Long
        (10500, 10500),  # exact tolerance boundary from Long
        (10501, 10501),  # just outside tolerance from Long
        (13500, 13700),  # mid=13600: distance 600 to D & E (> tolerance)
        (13500, 13600),  # mid=13550: distance 550
        (13100, 13900),  # mid=13500: distance 500 to D (wins over 700 to E)
        (13500, 13500),  # mid=13500: distance 500 to D
    ]
    for s, e in probes:
        expected = _linear_reference(s, e, turns)
        assert index.speaker_for_word(s, e) == expected
        assert transcript.speaker_for_word(s, e, turns) == expected

    # Exercise > 32 turns path with random overlapping & unsorted turns.
    rng = random.Random(42)
    many_turns: list[transcript.Turn] = []
    for i in range(80):
        s = rng.randint(0, 20_000)
        e = s + rng.randint(100, 4_000)
        many_turns.append((s, e, f"S{i % 7}"))
    many_index = transcript.TurnIndex(many_turns)
    for _ in range(300):
        s = rng.randint(-600, 25_000)
        e = s + rng.randint(0, 1_000)
        expected = _linear_reference(s, e, many_turns)
        assert many_index.speaker_for_word(s, e) == expected
        assert transcript.speaker_for_word(s, e, many_turns) == expected


def test_speaker_for_word_10k_turns_is_fast():
    # 10,000 turns across 10 hours (3.6 s each with slight overlap).
    turns: list[transcript.Turn] = [(i * 3600, i * 3600 + 3500, f"S{i % 4}") for i in range(10_000)]
    words = [word(f" w{i}", i * 600, i * 600 + 400) for i in range(50_000)]

    t0 = time.perf_counter()
    for w in words:
        transcript.speaker_for_word(w["start_ms"], w["end_ms"], turns)
    direct_elapsed = time.perf_counter() - t0
    assert direct_elapsed < 2.0

    t1 = time.perf_counter()
    doc = transcript.merge(
        "m10h",
        [{"name": "discord", "source": "app:discord.exe"}],
        {"discord": asr(*words)},
        {"discord": turns},
    )
    merge_elapsed = time.perf_counter() - t1
    assert merge_elapsed < 2.0
    assert len(doc["segments"]) > 0
