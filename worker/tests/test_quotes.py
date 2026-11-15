from bbb_worker import quotes
from bbb_worker.windows import Line


def line(
    start_s: float, end_s: float, text: str, speaker: str = "user", track: str = "mic"
) -> Line:
    return Line(int(start_s * 1000), int(end_s * 1000), track, speaker, speaker, text)


def corpus() -> quotes.Corpus:
    return quotes.Corpus.from_lines(
        [
            line(0, 5, "Okay, let's start with the budget for the monitor mount."),
            line(10, 18, "Alex, can you have the mount ready by next Friday?", "alex", "discord"),
            line(20, 24, "Yes, I will finish the prototype by Friday."),
            line(400, 410, "We agreed on 1 000 euro for the supplier, so five units are fine."),
            line(600, 606, "Zrobię to na piątek, bez problemu."),
        ]
    )


def test_normalize_handles_case_punctuation_diacritics_and_numerals():
    assert quotes.normalize("Zrobię to NA piątek!") == "zrobie to na piatek"
    assert quotes.normalize("Łódź, 1 000 euro") == "lodz 1000 euro"
    assert quotes.normalize("five units") == quotes.normalize("5 units")
    assert quotes.normalize("pięć sztuk") == quotes.normalize("5 sztuk")
    assert quotes.normalize("don't") == "dont"


def test_exact_quote_is_verified_with_the_position_of_its_line():
    found = quotes.verify("Alex, can you have the mount ready by next Friday?", 14_000, corpus())
    assert found.verified
    assert found.match == 1.0
    assert (found.t_start_ms, found.t_end_ms) == (10_000, 18_000)
    assert (found.track, found.speaker) == ("discord", "alex")


def test_quote_may_be_part_of_a_segment():
    found = quotes.verify("can you have the mount ready by next friday", 12_000, corpus())
    assert found.verified and found.match >= 0.95
    assert found.t_start_ms == 10_000


def test_quote_may_span_two_segments():
    found = quotes.verify(
        "ready by next Friday? Yes, I will finish the prototype", 15_000, corpus()
    )
    assert found.verified
    assert (found.t_start_ms, found.t_end_ms) == (10_000, 24_000)


def test_lost_diacritics_still_match():
    found = quotes.verify("Zrobie to na piatek bez problemu", 600_000, corpus())
    assert found.verified and found.match == 1.0
    assert found.t_start_ms == 600_000


def test_numerals_are_tolerated():
    found = quotes.verify(
        "We agreed on 1000 euro for the supplier, so 5 units are fine", 405_000, corpus()
    )
    assert found.verified and found.match >= 0.97


def test_reworded_quote_is_below_the_threshold():
    found = quotes.verify("Alex will deliver the mounting bracket next week", 14_000, corpus())
    assert not found.verified
    assert found.match < quotes.THRESHOLD
    # an unverified item is placed at the line nearest to the stated time
    assert (found.t_start_ms, found.t_end_ms) == (10_000, 18_000)


def test_right_text_at_the_wrong_time_is_found_elsewhere_in_the_window():
    found = quotes.verify("Zrobię to na piątek, bez problemu.", 15_000, corpus())
    assert found.verified
    assert found.t_start_ms == 600_000


def test_better_match_elsewhere_beats_a_weak_one_near_the_time():
    found = quotes.verify("I will finish the prototype by Friday", 405_000, corpus())
    assert found.verified
    assert found.t_start_ms == 20_000


def test_empty_corpus_and_empty_quote_are_unverified():
    empty = quotes.Corpus.from_lines([])
    assert not quotes.verify("anything", 1000, empty).verified
    assert not quotes.verify("...", 1000, corpus()).verified


def test_long_text_is_searched_quickly():
    long_lines = [
        line(i * 5, i * 5 + 4, f"filler sentence number {i} about nothing") for i in range(2000)
    ]
    long_lines[1500] = line(7500, 7504, "the supplier will deliver the sample on Thursday")
    c = quotes.Corpus.from_lines(long_lines)
    found = quotes.verify("the supplier will deliver the sample on Thursday", 100_000, c)
    assert found.verified and found.t_start_ms == 7_500_000
