import pytest

from bbb_tts import voices

CATALOG = [{"id": f"v{i:02d}", "gender": "f" if i % 2 else "m"} for i in range(20)]
CAST = ["alex", "kasia", "jan", "marta", "piotr", "ola", "tomek", "ewa"]


def test_same_seed_same_assignment():
    assert voices.assign_voices(CAST, CATALOG, 1) == voices.assign_voices(CAST, CATALOG, 1)


def test_input_order_does_not_matter():
    assert voices.assign_voices(CAST, CATALOG, 5) == voices.assign_voices(CAST[::-1], CATALOG, 5)


def test_voices_are_unique_within_a_meeting():
    for seed in range(30):
        got = voices.assign_voices(CAST, CATALOG, seed)
        assert len(set(got.values())) == len(CAST)


def test_seed_changes_assignment():
    assert any(
        voices.assign_voices(CAST, CATALOG, 1) != voices.assign_voices(CAST, CATALOG, s)
        for s in range(2, 8)
    )


def test_gender_follows_name_when_known():
    by_id = {v["id"]: v["gender"] for v in CATALOG}
    got = voices.assign_voices(["kasia", "jan", "marta", "piotr"], CATALOG, 2)
    assert by_id[got["kasia"]] == "f" and by_id[got["marta"]] == "f"
    assert by_id[got["jan"]] == "m" and by_id[got["piotr"]] == "m"


def test_gender_clash_falls_back_to_other_gender():
    only_f = [{"id": "f1", "gender": "f"}, {"id": "f2", "gender": "f"}]
    got = voices.assign_voices(["jan", "piotr"], only_f, 1)
    assert sorted(got.values()) == ["f1", "f2"]


def test_too_many_speakers_rejected():
    with pytest.raises(ValueError):
        voices.assign_voices([f"s{i}" for i in range(21)], CATALOG, 1)


def test_stable_voice_when_cast_changes_without_clash():
    small = voices.assign_voices(["kasia", "jan"], CATALOG, 9)
    big = voices.assign_voices(["kasia", "jan", "ola"], CATALOG, 9)
    # Sorted order is jan, kasia, ola: ola is served last and cannot steal earlier choices.
    assert small["jan"] == big["jan"] and small["kasia"] == big["kasia"]


def test_shipped_catalog_is_consistent():
    catalog = voices.load_catalog()
    ids = [v["id"] for v in catalog]
    assert len(ids) == len(set(ids)) == 20
    assert {v["gender"] for v in catalog} == {"f", "m"}
