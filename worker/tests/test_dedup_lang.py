from bbb_worker import dedup, lang


def item(text, window, start_s, *, type="task", assignee=None, due=None, verified=True, match=1.0):
    return {
        "type": type,
        "text": text,
        "assignee": assignee,
        "due": due,
        "certainty": {
            "assignee": "stated" if assignee else "unknown",
            "due": "stated" if due else "unknown",
        },
        "section": None,
        "relates_to": None,
        "window": window,
        "source": {
            "track": "mic",
            "speaker": "user",
            "t_start_ms": start_s * 1000,
            "t_end_ms": start_s * 1000 + 5000,
            "quote": text,
            "verified": verified,
            "match": match,
        },
    }


def test_duplicates_across_the_overlap_collapse():
    a = item("Alex prepares the monitor mount design", 0, 890)
    b = item("Alex prepares the monitor mount design.", 1, 892)
    merged = dedup.merge_duplicates([a, b])
    assert len(merged) == 1


def test_distinct_items_stay():
    a = item("Alex prepares the monitor mount design", 0, 890)
    b = item("Sam orders the aluminium profiles", 1, 892)
    c = item("Alex prepares the monitor mount design", 1, 4000)  # same text, far in time
    d = item("Alex prepares the monitor mount design", 1, 893, type="decision")
    assert len(dedup.merge_duplicates([a, b, c, d])) == 4


def test_items_of_one_window_are_never_merged():
    a = item("Alex prepares the monitor mount design", 0, 100)
    b = item("Alex prepares the monitor mount design", 0, 105)
    assert len(dedup.merge_duplicates([a, b])) == 2


def test_containment_merges_a_shortened_statement():
    a = item("Order the aluminium profiles for the mount", 0, 890)
    b = item("Order the aluminium profiles for the mount from the new supplier", 1, 895)
    assert len(dedup.merge_duplicates([a, b])) == 1


def test_short_containment_does_not_merge():
    a = item("Fix it", 0, 890)
    b = item("Fix it in the staging environment before the release", 1, 895)
    assert len(dedup.merge_duplicates([a, b])) == 2


def test_verified_item_wins_and_takes_over_missing_fields():
    unverified = item(
        "Alex prepares the mount design",
        0,
        890,
        assignee="alex",
        due="2026-11-20",
        verified=False,
        match=0.5,
    )
    verified = item("Alex prepares the mount design", 1, 892)
    (merged,) = dedup.merge_duplicates([unverified, verified])
    assert merged["source"]["verified"] is True
    assert merged["assignee"] == "alex" and merged["due"] == "2026-11-20"
    assert merged["certainty"] == {"assignee": "stated", "due": "stated"}


def test_more_complete_item_wins_and_certainty_takes_the_best():
    a = item("Alex prepares the mount design", 0, 890, assignee="alex")
    a["certainty"]["assignee"] = "inferred"
    b = item("Alex prepares the mount design", 1, 892, assignee="alex", due="2026-11-20")
    (merged,) = dedup.merge_duplicates([a, b])
    assert merged["due"] == "2026-11-20"
    assert merged["certainty"]["assignee"] == "stated"


def test_merge_does_not_mutate_the_inputs():
    a = item("Alex prepares the mount design", 0, 890)
    b = item("Alex prepares the mount design", 1, 892, assignee="alex")
    dedup.merge_duplicates([a, b])
    assert a["assignee"] is None and a["certainty"]["assignee"] == "unknown"


def test_result_is_ordered_by_time():
    items = [item("third thing to do soon", 0, 500), item("first thing to do", 0, 10)]
    assert [i["text"] for i in dedup.merge_duplicates(items)] == [
        "first thing to do",
        "third thing to do soon",
    ]


# Language ----------------------------------------------------------------------------------

PL = [
    "Dobrze, ustalmy to jeszcze dzisiaj, bo mamy mało czasu.",
    "Zrobię to na piątek, jak będzie można.",
]
EN = [
    "Okay, we need to decide this today because we are running out of time.",
    "I will do it by Friday.",
]


def test_explicit_language_wins():
    assert lang.resolve("pl", "en", EN) == "pl"
    assert lang.resolve("en", "pl", PL) == "en"


def test_auto_follows_the_text():
    assert lang.resolve("auto", "und", PL) == "pl"
    assert lang.resolve("auto", "und", EN) == "en"
    assert lang.resolve("auto", "en", PL) == "pl"  # ASR said English, the text is Polish


def test_auto_uses_the_asr_language_when_the_text_does_not_decide():
    assert lang.resolve("auto", "pl", ["ok", "mhm"]) == "pl"
    assert lang.resolve("auto", "en", []) == "en"


def test_auto_tie_goes_to_english():
    assert lang.resolve("auto", "und", []) == "en"
    assert lang.resolve("auto", "de", ["xyz"]) == "en"


def test_mixed_speech_with_a_clear_majority():
    mixed = [*PL, *PL, EN[1]]
    assert lang.resolve("auto", "en", mixed) == "pl"
