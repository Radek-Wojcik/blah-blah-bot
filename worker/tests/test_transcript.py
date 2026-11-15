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
