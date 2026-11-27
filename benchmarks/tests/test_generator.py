from datetime import date

import pytest

from bbb_bench.datasets import generator, pools


def test_day_ordinals():
    assert pools.day_pl(1) == "pierwszego"
    assert pools.day_pl(13) == "trzynastego"
    assert pools.day_pl(20) == "dwudziestego"
    assert pools.day_pl(21) == "dwudziestego pierwszego"
    assert pools.day_pl(31) == "trzydziestego pierwszego"
    assert pools.day_en(2) == "second"
    assert pools.day_en(12) == "twelfth"
    assert pools.day_en(30) == "thirtieth"
    assert pools.day_en(23) == "twenty third"


def test_generation_is_deterministic():
    a = generator.generate_meeting("pl", 4, 5, seed=7)
    b = generator.generate_meeting("pl", 4, 5, seed=7)
    assert a.turns == b.turns and a.tasks == b.tasks


def test_different_seeds_differ():
    a = generator.generate_meeting("pl", 4, 5, seed=1)
    b = generator.generate_meeting("pl", 4, 5, seed=2)
    assert a.turns != b.turns


@pytest.mark.parametrize(("lang", "mixed"), [("pl", False), ("en", False), ("pl", True)])
def test_annotations_point_at_their_turns(lang, mixed):
    m = generator.generate_meeting(lang, 5, 10, seed=3, mixed=mixed)
    assert m.tasks and m.decisions
    for item in m.tasks + m.decisions + m.budgets:
        assert m.turns[item["turn"]]["text"] == item["quote"]
    for task in m.tasks:
        assert task["assignee"] in m.speakers or (
            task["assignee"] is None and bool(task.get("assignee_name"))
        )
        if task["due"] is not None:
            due = date.fromisoformat(task["due"])
            assert due.weekday() < 5 and due > m.meeting_date


def test_duration_target_is_met():
    m = generator.generate_meeting("en", 3, 10, seed=5)
    assert m.words() >= 10 * generator.WORDS_PER_MINUTE


def test_speaker_count_validation():
    with pytest.raises(ValueError):
        generator.generate_meeting("pl", 1, 5, seed=1)
    with pytest.raises(ValueError):
        generator.generate_meeting("pl", 99, 5, seed=1)
    with pytest.raises(ValueError):
        generator.generate_meeting("de", 3, 5, seed=1)


def test_mixed_meeting_contains_glossary_terms_and_english_turns():
    m = generator.generate_meeting("pl", 4, 30, seed=11, mixed=True)
    text = " ".join(t["text"] for t in m.turns)
    assert any(term.lower() in text.lower() for term in pools.GLOSSARY)
    assert {t["lang"] for t in m.turns} == {"pl", "en"}


def test_files_written(tmp_path):
    m = generator.generate_meeting("pl", 3, 3, seed=2, mixed=True)
    generator.to_files(m, tmp_path / "m", pools.GLOSSARY)
    names = {p.name for p in (tmp_path / "m").iterdir()}
    assert names == {"script.json", "annotation.json", "reference.txt", "transcript.txt"}


def test_fixed_people_are_used_in_order():
    m = generator.generate_meeting("pl", 3, 3, seed=4, people=["alex", "kasia", "jan"])
    assert m.speakers == ["alex", "kasia", "jan"]


def test_fixed_people_validation():
    with pytest.raises(ValueError):
        generator.generate_meeting("pl", 3, 3, seed=4, people=["alex", "nobody", "jan"])
    with pytest.raises(ValueError):
        generator.generate_meeting("pl", 3, 3, seed=4, people=["alex", "kasia"])
