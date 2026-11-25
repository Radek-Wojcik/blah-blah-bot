from bbb_bench import langid


def test_polish_with_diacritics():
    assert langid.detect("Dobrze, ja to zrobię do środy.") == "pl"


def test_polish_without_diacritics():
    assert langid.detect("Tak, to jest dobrze i nie mam pytań") == "pl"


def test_english():
    assert langid.detect("Sure, I will have it done by Friday.") == "en"


def test_mixed_polish_sentence_with_english_terms_is_polish():
    assert langid.detect("Firmware jest już wgrany na dwie płytki.") == "pl"


def test_tie_without_evidence_is_unknown_not_english():
    assert langid.detect("Kubernetes 42") == "unknown"
    assert langid.detect("") == "unknown"


def test_equal_nonzero_scores_are_unknown():
    assert langid.detect("the nie") == "unknown"
