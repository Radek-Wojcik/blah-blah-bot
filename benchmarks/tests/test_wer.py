import pytest

from bbb_bench.metrics import normalize, rtfx, term_recall, translation_rate, wer


def test_normalize_keeps_polish_letters_and_drops_punctuation():
    assert normalize("Zażółć, GĘŚLĄ jaźń!") == ["zażółć", "gęślą", "jaźń"]


def test_normalize_joins_thousands_and_splits_hyphens():
    assert normalize("Budżet: 12 000 zł, pre-test") == ["budżet", "12000", "zł", "pre", "test"]


def test_wer_hand_computed_one_substitution():
    # 4 reference words, one substitution -> 1/4
    r = wer("the cat sat down", "the cat sit down")
    assert r["wer"] == pytest.approx(0.25)
    assert (r["substitutions"], r["deletions"], r["insertions"]) == (1, 0, 0)


def test_wer_hand_computed_mixed_errors():
    # ref: a b c d e (5 words); hyp: a x c e f
    # b->x substitution, d deleted, f inserted => S=1 D=1 I=1 => 3/5
    r = wer("a b c d e", "a x c e f")
    assert r["wer"] == pytest.approx(0.6)
    assert (r["substitutions"], r["deletions"], r["insertions"]) == (1, 1, 1)


def test_wer_ignores_case_and_punctuation():
    assert wer("Hello, World.", "hello world")["wer"] == 0.0


def test_wer_can_exceed_one():
    assert wer("a", "b c d")["wer"] == pytest.approx(3.0)


def test_wer_empty_reference():
    assert wer("", "")["wer"] == 0.0
    assert wer("", "x")["wer"] == 1.0


def test_wer_matches_jiwer_on_sample():
    jiwer = pytest.importorskip("jiwer")
    ref = "spotkanie zaczyna się o dziesiątej w sali konferencyjnej"
    hyp = "spotkanie zaczyna sie o dziesiątej sali konferencyjnej dzisiaj"
    assert wer(ref, hyp)["wer"] == pytest.approx(jiwer.wer(ref, hyp))


def test_term_recall_counts_occurrences():
    # "CAN bus" planted twice but found once, GNSS found => 2/3
    r = term_recall(["CAN bus", "CAN bus", "GNSS"], "we use the can bus and gnss here")
    assert r["term_recall"] == pytest.approx(2 / 3)


def test_term_recall_requires_whole_words():
    assert term_recall(["bus"], "omnibus")["term_recall"] == 0.0


def test_translation_rate():
    result = translation_rate(["pl", "pl", "en", "pl"], ["pl", "en", "en", "pl"])
    assert result["translation_rate"] == pytest.approx(0.25)


def test_translation_rate_length_mismatch():
    with pytest.raises(ValueError):
        translation_rate(["pl"], [])


def test_rtfx():
    assert rtfx(3600, 60) == 60
    with pytest.raises(ValueError):
        rtfx(1, 0)
