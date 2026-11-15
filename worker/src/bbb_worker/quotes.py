"""Verification of quotes against the transcript.

The model must copy a quote verbatim from the line where an item is stated. It sometimes
paraphrases, and it sometimes states the wrong time. A quote is *verified* when it matches
transcript text closely enough, which is the guard against invented items.

Matching works on normalised text: case-folded, Polish and other diacritics stripped (ASR and
models lose them inconsistently), punctuation removed, thousand separators joined and small
spelled-out numbers written as digits (ASR writes "5", models write "five").

Similarity is the `difflib` ratio of the normalised quote and the candidate transcript text,
measured on characters. Token overlap is used only to find the few candidate positions in a
long text, because it is cheap. The character ratio is the verdict because it tolerates
what ASR and models really change (an ending, a dropped letter, a missing article) but falls
quickly when words are replaced, so a reworded quote (a paraphrase) scores below the
threshold. Candidates are runs of consecutive tokens, so a quote may be part of a segment or
span several segments.

Only speech counts as transcript: screenshot descriptions are model text and are left out of
the corpus. A quote with fewer than MIN_QUOTE_TOKENS words or MIN_QUOTE_CHARS normalised
characters ("yes", "okay, done") fits almost any conversation, so it is verified only when it
equals the whole text of one segment; otherwise its match is capped below the threshold.
"""

# ruff: noqa: SIM905  (word lists are written as one string and split)
from __future__ import annotations

import re
import unicodedata
from collections import Counter
from collections.abc import Sequence
from dataclasses import dataclass
from difflib import SequenceMatcher

from .windows import SPEECH, Line

THRESHOLD = 0.85
NEAR_MS = 90_000
# A near match this good is final; anything weaker is compared with the whole window.
GOOD_ENOUGH = 0.98
MIN_QUOTE_TOKENS = 3
MIN_QUOTE_CHARS = 12
_TOP_STARTS = 3
_SLACK = 2

_THOUSANDS = re.compile(r"(?<=\d)[  ,.](?=\d{3}\b)")
_DROPPED = re.compile(r"['’_]")
_PUNCTUATION = re.compile(r"[^\w\s]")

_NUMBER_WORDS = {
    **{
        w: str(i)
        for i, w in enumerate(
            "zero one two three four five six seven eight nine ten eleven twelve thirteen "
            "fourteen fifteen sixteen seventeen eighteen nineteen twenty".split()
        )
    },
    **{
        w: str(i)
        for i, w in enumerate(
            "zero jeden dwa trzy cztery piec szesc siedem osiem dziewiec dziesiec jedenascie "
            "dwanascie trzynascie czternascie pietnascie szesnascie siedemnascie osiemnascie "
            "dziewietnascie dwadziescia".split()
        )
    },
    "jedna": "1",
    "jedno": "1",
    "dwie": "2",
}


def normalize(text: str) -> str:
    decomposed = unicodedata.normalize("NFKD", text.casefold()).replace("ł", "l")
    stripped = "".join(c for c in decomposed if not unicodedata.combining(c))
    stripped = _THOUSANDS.sub("", stripped)
    stripped = _PUNCTUATION.sub(" ", _DROPPED.sub("", stripped))
    return " ".join(_NUMBER_WORDS.get(w, w) for w in stripped.split())


@dataclass(frozen=True)
class Corpus:
    """The normalised tokens of a window's lines, with the line each token belongs to."""

    lines: tuple[Line, ...]
    tokens: tuple[str, ...]
    owner: tuple[int, ...]

    @classmethod
    def from_lines(cls, lines: Sequence[Line]) -> Corpus:
        tokens: list[str] = []
        owner: list[int] = []
        lines = [ln for ln in lines if ln.kind == SPEECH]
        for i, line in enumerate(lines):
            words = normalize(line.text).split()
            tokens.extend(words)
            owner.extend([i] * len(words))
        return cls(tuple(lines), tuple(tokens), tuple(owner))

    def token_range(self, lo_ms: int, hi_ms: int) -> tuple[int, int]:
        """Token index range of the lines that overlap `[lo_ms, hi_ms]`."""
        hit = [i for i, ln in enumerate(self.lines) if ln.end_ms >= lo_ms and ln.start_ms <= hi_ms]
        if not hit:
            return (0, 0)
        indexes = [k for k, o in enumerate(self.owner) if o in range(hit[0], hit[-1] + 1)]
        return (indexes[0], indexes[-1] + 1) if indexes else (0, 0)


@dataclass(frozen=True)
class Match:
    score: float
    first_token: int
    last_token: int  # exclusive


def _candidate_starts(query: Sequence[str], tokens: Sequence[str], lo: int, hi: int) -> list[int]:
    """Start positions of the query-sized token runs that share the most tokens with it."""
    size = len(query)
    need = Counter(query)
    have: Counter[str] = Counter()
    overlap = 0
    scored: list[tuple[int, int]] = []
    for i in range(lo, hi):
        tok = tokens[i]
        have[tok] += 1
        if have[tok] <= need[tok]:
            overlap += 1
        if i - lo >= size:
            old = tokens[i - size]
            if have[old] <= need[old]:
                overlap -= 1
            have[old] -= 1
        if i - lo >= size - 1:
            scored.append((-overlap, i - size + 1))
    scored.sort()
    return [start for _score, start in scored[:_TOP_STARTS]]


def _ratio(a: str, b: str) -> float:
    return SequenceMatcher(None, a, b, autojunk=False).ratio()


def best_match(query: str, corpus: Corpus, lo: int, hi: int) -> Match | None:
    """Best matching run of tokens in `corpus.tokens[lo:hi]`, or None when there is no text."""
    q_text = normalize(query)
    q_tokens = q_text.split()
    if not q_tokens or hi <= lo:
        return None
    size = len(q_tokens)
    if hi - lo <= size + _SLACK:
        starts = [lo]
    else:
        starts = _candidate_starts(q_tokens, corpus.tokens, lo, hi)
    best: Match | None = None
    for start in starts:
        for ds in range(-_SLACK, _SLACK + 1):
            for dl in range(-_SLACK, _SLACK + 1):
                a = max(lo, start + ds)
                b = min(hi, start + ds + size + dl)
                if b <= a:
                    continue
                score = _ratio(q_text, " ".join(corpus.tokens[a:b]))
                if best is None or score > best.score:
                    best = Match(score, a, b)
    return best


@dataclass(frozen=True)
class Verification:
    verified: bool
    match: float
    t_start_ms: int
    t_end_ms: int
    track: str
    speaker: str


def _nearest_line(corpus: Corpus, time_ms: int) -> Line | None:
    if not corpus.lines:
        return None
    return min(
        corpus.lines,
        key=lambda ln: (
            0
            if ln.start_ms <= time_ms <= ln.end_ms
            else min(abs(ln.start_ms - time_ms), abs(ln.end_ms - time_ms))
        ),
    )


def _is_short(q_text: str) -> bool:
    return len(q_text.split()) < MIN_QUOTE_TOKENS or len(q_text) < MIN_QUOTE_CHARS


def _whole_line(q_text: str, time_ms: int, corpus: Corpus) -> Line | None:
    """The line (nearest to `time_ms`) whose whole normalised text is `q_text`."""
    same = [ln for ln in corpus.lines if normalize(ln.text) == q_text]
    if not same:
        return None
    return min(same, key=lambda ln: min(abs(ln.start_ms - time_ms), abs(ln.end_ms - time_ms)))


def verify(quote: str, time_ms: int, corpus: Corpus, threshold: float = THRESHOLD) -> Verification:
    """Match `quote` near `time_ms` first, then in the whole window; keep the better match.

    A verified item gets the position of the matched lines. An unverified one gets the line
    nearest to the stated time, because a poor match says nothing about where the item is.
    A short quote is verified only as the whole text of a line (see the module docstring).
    """
    q_text = normalize(quote)
    short = _is_short(q_text)
    if short and (whole := _whole_line(q_text, time_ms, corpus)) is not None:
        return Verification(True, 1.0, whole.start_ms, whole.end_ms, whole.track, whole.speaker)
    near = best_match(quote, corpus, *corpus.token_range(time_ms - NEAR_MS, time_ms + NEAR_MS))
    best = near
    if best is None or best.score < GOOD_ENOUGH:
        anywhere = best_match(quote, corpus, 0, len(corpus.tokens))
        if anywhere is not None and (best is None or anywhere.score > best.score):
            best = anywhere
    score = best.score if best else 0.0
    if short:
        score = min(score, round(threshold - 0.01, 4))
    if best is not None and score >= threshold:
        first = corpus.lines[corpus.owner[best.first_token]]
        last = corpus.lines[corpus.owner[best.last_token - 1]]
        return Verification(
            True, round(score, 4), first.start_ms, last.end_ms, first.track, first.speaker
        )
    line = _nearest_line(corpus, time_ms)
    if line is None:
        return Verification(False, round(score, 4), time_ms, time_ms, "", "")
    return Verification(
        False, round(score, 4), line.start_ms, line.end_ms, line.track, line.speaker
    )
