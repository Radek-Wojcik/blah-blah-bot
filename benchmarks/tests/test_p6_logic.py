from bbb_bench.proto import p6_vision


def test_facts_present_whole_words_only():
    found = p6_vision.facts_present(
        "The chart shows 22 tasks in W45 and 140 later.", ["22", "W45", "14"]
    )
    assert found == ["22", "W45"]


def test_facts_with_units():
    assert p6_vision.facts_present("Torque was 48 Nm.", ["48 Nm"]) == ["48 Nm"]


def test_invented_numbers():
    assert p6_vision.invented_numbers("12 of 14 parts and 99 more", ["12", "14"]) == ["99"]
