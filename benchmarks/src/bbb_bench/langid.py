"""Tiny Polish/English language identifier for transcript segments.

Used only to score the translation rate: it compares the language of a ASR
output segment with the language it was spoken in. It relies on diacritics and
common function words, which is reliable for the short, plain sentences of the
scripted test sets and avoids another model dependency.
"""

from __future__ import annotations

from .metrics.text import normalize

_PL_WORDS = frozenset(
    "i w z na do nie to jest się że jak ale czy ja ty my wy oni tak dobrze zrobię zrobi może "
    "mam za dużo głowie niech dziękuję zgoda wszystkim dzień dobry spotkanie budżet złotych "
    "ustalmy proszę będzie dla przez po od o u co ten ta te tym już jeszcze".split()
)
_EN_WORDS = frozenset(
    "the and of to in is it that for you we are be on with as this have will can not but "
    "i will sure great thank you thanks budget euros agreed okay noted let us do by".split()
)
_PL_CHARS = set("ąćęłńóśźż")
UNKNOWN = "unknown"


def detect(text: str) -> str:
    """Return 'pl', 'en' or 'unknown' for a segment of text.

    A tie (including 0 against 0, e.g. a segment made only of proper names or
    numbers) is 'unknown': it must count neither as English nor as a translation.
    """
    low = text.lower()
    words = normalize(text)
    pl = sum(1 for w in words if w in _PL_WORDS) + 2 * sum(1 for c in low if c in _PL_CHARS)
    en = sum(1 for w in words if w in _EN_WORDS)
    if pl == en:
        return UNKNOWN
    return "pl" if pl > en else "en"
