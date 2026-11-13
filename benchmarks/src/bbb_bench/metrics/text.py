"""Text normalization shared by the metrics."""

from __future__ import annotations

import re
import unicodedata

_PUNCT = re.compile(r"[^\w\s]", re.UNICODE)
_SPACES = re.compile(r"\s+")
_THOUSANDS = re.compile(r"(?<=\d)[  ,.](?=\d{3}\b)")


def normalize(text: str) -> list[str]:
    """Lowercase, remove punctuation, collapse whitespace; return word tokens.

    Polish letters are preserved (NFC). Digit groups written with thousand
    separators are joined ("1 000" -> "1000"). Spelled-out numbers are not
    converted: test scripts write numbers consistently, see benchmarks/README.md.
    """
    text = unicodedata.normalize("NFC", text).lower()
    text = _THOUSANDS.sub("", text)
    text = text.replace("-", " ").replace("_", " ")
    text = _PUNCT.sub("", text)
    return _SPACES.sub(" ", text).strip().split()
