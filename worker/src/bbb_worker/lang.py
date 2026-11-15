"""Output language of the report (`pl` or `en`).

`auto` resolves to the dominant language of the transcript. The language the ASR reported is
a good prior but not reliable for mixed speech, so the text itself is checked: stop words
that belong to only one of the two languages, and Polish letters, are counted. A clear winner
by text decides; otherwise the ASR language decides; with neither, English.
"""

# ruff: noqa: SIM905  (word lists are written as one string and split)
from __future__ import annotations

import re
from collections.abc import Iterable

SUPPORTED = ("pl", "en")
NAMES = {"pl": "Polish", "en": "English"}

# Words that are not common in the other language (shared words like "to", "a", "on", "do",
# "no" and "i" are left out on purpose).
_PL_WORDS = frozenset(
    "nie się jest że jak ale tak czy dla mamy będzie może jeszcze tylko już bardzo ten tego "
    "który dobrze przez są był było więc też żeby bo się mnie mi cię tu tam teraz zrobić "
    "zrobię zrobimy ustalmy dzisiaj jutro wszystko wszyscy okej dobra".split()
)
_EN_WORDS = frozenset(
    "the and of is that it for we you this are with be have not but will can they what just "
    "about so there their would should then okay yeah right going need let's i'll don't "
    "we'll today tomorrow everyone".split()
)
_POLISH_LETTERS = re.compile(r"[ąćęłńóśźż]")
_WORD = re.compile(r"[^\W\d_]+(?:'[^\W\d_]+)?", re.UNICODE)
# One language must lead the other by this factor to win on text alone.
_MARGIN = 1.25


def score(texts: Iterable[str]) -> tuple[float, float]:
    """Polish and English evidence per word of the text (not comparable across calls)."""
    pl = en = 0
    words = 0
    for text in texts:
        lowered = text.casefold()
        for w in _WORD.findall(lowered):
            words += 1
            pl += w in _PL_WORDS
            en += w in _EN_WORDS
        pl += len(_POLISH_LETTERS.findall(lowered)) / 4
    if not words:
        return (0.0, 0.0)
    return (pl / words, en / words)


def dominant(texts: Iterable[str], asr_language: str = "und") -> str:
    pl, en = score(texts)
    if pl > en * _MARGIN and pl > 0:
        return "pl"
    if en > pl * _MARGIN and en > 0:
        return "en"
    return asr_language if asr_language in SUPPORTED else "en"


def resolve(requested: str, asr_language: str, texts: Iterable[str]) -> str:
    """The language the draft is written in."""
    if requested in SUPPORTED:
        return requested
    return dominant(texts, asr_language)
