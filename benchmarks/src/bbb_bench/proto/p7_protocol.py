"""Pure logic for the P7 receiver: frame and control parsing, WAV writing, gap metrics, verdicts.

Nothing here needs a network or a browser, so it is covered by unit tests. The wire format is
described in `benchmarks/p7-chrome/README.md`.
"""

from __future__ import annotations

import hmac
import json
import re
import secrets
import struct
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

PROTOCOL_VERSION = 1
SAMPLE_RATE = 16000
CHANNELS = 1
SAMPLE_WIDTH = 2
GAP_LIMIT_MS = 100.0
LONG_RUN_S = 30 * 60
# Stable extension id, derived from the public `key` in the extension manifest.
DEFAULT_EXTENSION_ID = "mddhpjgdobfbknicbomoojpbpkgfpima"

HEADER = struct.Struct("<IId")  # tab_id, seq, sender timestamp (epoch ms)
HEADER_BYTES = HEADER.size
_ORIGIN_RE = re.compile(r"chrome-extension://([a-p]{32})")
_TOKEN_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"


class ProtocolError(ValueError):
    """A message that violates the wire format."""


# --- frames and control messages -------------------------------------------------------------


@dataclass(frozen=True)
class FrameHeader:
    tab_id: int
    seq: int
    sent_ms: float
    payload: bytes

    @property
    def samples(self) -> int:
        return len(self.payload) // SAMPLE_WIDTH


def parse_frame(data: bytes) -> FrameHeader:
    """Split a binary frame into header fields and PCM payload (signed 16-bit mono)."""
    if len(data) < HEADER_BYTES:
        raise ProtocolError(f"frame shorter than the {HEADER_BYTES}-byte header")
    tab_id, seq, sent_ms = HEADER.unpack_from(data)
    payload = bytes(data[HEADER_BYTES:])
    if len(payload) % SAMPLE_WIDTH:
        raise ProtocolError("payload is not a whole number of 16-bit samples")
    return FrameHeader(tab_id, seq, sent_ms, payload)


def pack_frame(tab_id: int, seq: int, sent_ms: float, payload: bytes) -> bytes:
    """Inverse of `parse_frame` (used by tests and the Python fake client)."""
    return HEADER.pack(tab_id, seq, sent_ms) + payload


def _require_int(msg: dict[str, Any], key: str, lo: int, hi: int) -> int:
    value = msg.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or not lo <= value <= hi:
        raise ProtocolError(f"field {key!r} must be an integer in [{lo}, {hi}]")
    return value


def parse_control(text: str | bytes) -> dict[str, Any]:
    """Validate a JSON control message (`hello`, `start`, `stop`) and return it as a dict."""
    try:
        msg = json.loads(text)
    except (ValueError, TypeError) as exc:
        raise ProtocolError(f"invalid JSON: {exc}") from exc
    if not isinstance(msg, dict):
        raise ProtocolError("control message must be a JSON object")
    kind = msg.get("type")
    if kind == "hello":
        _require_int(msg, "version", PROTOCOL_VERSION, PROTOCOL_VERSION)
        token, session = msg.get("token"), msg.get("session")
        if (token is None) == (session is None):
            raise ProtocolError("hello needs exactly one of 'token' or 'session'")
        if not isinstance(token if token is not None else session, str):
            raise ProtocolError("token/session must be a string")
    elif kind == "start":
        _require_int(msg, "tab_id", 0, 2**32 - 1)
        if _require_int(msg, "sample_rate", 1, 192000) != SAMPLE_RATE:
            raise ProtocolError(f"only {SAMPLE_RATE} Hz is supported")
        if _require_int(msg, "channels", 1, 8) != CHANNELS:
            raise ProtocolError("only mono is supported")
        if msg.get("format") != "s16le":
            raise ProtocolError("only format 's16le' is supported")
    elif kind == "stop":
        _require_int(msg, "tab_id", 0, 2**32 - 1)
        if not isinstance(msg.get("reason", ""), str):
            raise ProtocolError("reason must be a string")
    else:
        raise ProtocolError(f"unknown message type {kind!r}")
    return msg


# --- origin and pairing ------------------------------------------------------------------------


def origin_allowed(origin: str | None, extension_id: str | None = DEFAULT_EXTENSION_ID) -> bool:
    """Only the extension's own origin may connect; `extension_id=None` accepts any extension."""
    if not origin:
        return False
    match = _ORIGIN_RE.fullmatch(origin)
    if match is None:
        return False
    return extension_id is None or match.group(1) == extension_id


def new_token(length: int = 8) -> str:
    return "".join(secrets.choice(_TOKEN_ALPHABET) for _ in range(length))


class PairingStore:
    """One-time pairing token; after use the connection gets a session secret for reconnects."""

    def __init__(self, token: str):
        self._token: str | None = token
        self._session: str | None = None

    @property
    def paired(self) -> bool:
        return self._session is not None

    def claim_token(self, presented: str) -> str | None:
        """Consume the token. Returns a new session secret, or None if wrong or already used."""
        if self._token is None or not hmac.compare_digest(presented, self._token):
            return None
        self._token = None
        self._session = secrets.token_urlsafe(24)
        return self._session

    def check_session(self, presented: str) -> bool:
        return self._session is not None and hmac.compare_digest(presented, self._session)


# --- WAV ---------------------------------------------------------------------------------------


def wav_header(data_bytes: int, rate: int = SAMPLE_RATE, channels: int = CHANNELS) -> bytes:
    """44-byte canonical PCM WAV header for `data_bytes` of 16-bit audio."""
    block = channels * SAMPLE_WIDTH
    return (
        b"RIFF"
        + struct.pack("<I", 36 + data_bytes)
        + b"WAVEfmt "
        + struct.pack("<IHHIIHH", 16, 1, channels, rate, rate * block, block, 16)
        + b"data"
        + struct.pack("<I", data_bytes)
    )


class WavWriter:
    """Appends PCM to a WAV file; the header is patched on `flush_header` and `close`.

    A crash therefore leaves a file whose header is at most one flush interval stale.
    """

    def __init__(self, path: Path, rate: int = SAMPLE_RATE, channels: int = CHANNELS):
        self.path = path
        self.rate = rate
        self.channels = channels
        self.data_bytes = 0
        self._fh = open(path, "wb")  # noqa: SIM115 - closed explicitly in close()
        self._fh.write(wav_header(0, rate, channels))

    def write(self, pcm: bytes) -> None:
        self._fh.write(pcm)
        self.data_bytes += len(pcm)

    def flush_header(self) -> None:
        self._fh.flush()
        end = self._fh.tell()
        self._fh.seek(0)
        self._fh.write(wav_header(self.data_bytes, self.rate, self.channels))
        self._fh.seek(end)
        self._fh.flush()

    def close(self) -> None:
        if not self._fh.closed:
            self.flush_header()
            self._fh.close()


# --- gap metrics -------------------------------------------------------------------------------


@dataclass
class GapTracker:
    """Frame arrival statistics against the wall clock and against the sample clock.

    * `max_gap_wall_ms`: longest time between two consecutive frame arrivals.
    * `max_gap_samples_ms`: longest stall beyond the audio the previous frame carried, i.e.
      arrival interval minus the duration of the samples that arrived in the previous frame.
      Frames delivered in a burst after a stall do not hide it, because the stall itself is
      counted when it happens.
    * `drift_ms`: wall time elapsed minus audio time delivered over the whole stream
      (positive: audio arrives slower than real time).
    * `max_gap_sender_ms`: longest interval between sender timestamps (stalls on the capture side).
    """

    rate: int = SAMPLE_RATE
    gap_report_ms: float = 50.0
    frames: int = 0
    samples: int = 0
    frame_seq_lost: int = 0
    max_gap_wall_ms: float = 0.0
    max_gap_samples_ms: float = 0.0
    max_gap_sender_ms: float = 0.0
    first_arrival: float | None = None
    last_arrival: float | None = None
    _last_samples: int = 0
    _last_sent_ms: float | None = None
    _next_seq: int | None = None
    gaps: list[tuple[float, float]] = field(default_factory=list)  # (t since start s, gap ms)

    def update(self, arrival_s: float, samples: int, seq: int, sent_ms: float) -> None:
        if self.first_arrival is None:
            self.first_arrival = arrival_s
        if self.last_arrival is not None:
            gap_ms = (arrival_s - self.last_arrival) * 1000.0
            stall_ms = gap_ms - self._last_samples * 1000.0 / self.rate
            self.max_gap_wall_ms = max(self.max_gap_wall_ms, gap_ms)
            self.max_gap_samples_ms = max(self.max_gap_samples_ms, stall_ms)
            if gap_ms >= self.gap_report_ms and len(self.gaps) < 1000:
                self.gaps.append((self.last_arrival - self.first_arrival, gap_ms))
        if self._last_sent_ms is not None:
            self.max_gap_sender_ms = max(self.max_gap_sender_ms, sent_ms - self._last_sent_ms)
        if self._next_seq is not None and seq > self._next_seq:
            self.frame_seq_lost += seq - self._next_seq
        self._next_seq = seq + 1
        self.last_arrival = arrival_s
        self._last_samples = samples
        self._last_sent_ms = sent_ms
        self.frames += 1
        self.samples += samples

    @property
    def audio_duration_s(self) -> float:
        return self.samples / self.rate

    @property
    def wall_duration_s(self) -> float:
        if self.first_arrival is None or self.last_arrival is None:
            return 0.0
        return self.last_arrival - self.first_arrival

    @property
    def drift_ms(self) -> float:
        if self.frames < 2:
            return 0.0
        # The last frame's own audio has not elapsed yet at its arrival time.
        elapsed_audio = (self.samples - self._last_samples) / self.rate
        return (self.wall_duration_s - elapsed_audio) * 1000.0

    def gap_ok(self, limit_ms: float = GAP_LIMIT_MS) -> bool:
        return (
            self.frames > 0
            and self.max_gap_wall_ms <= limit_ms
            and self.max_gap_samples_ms <= limit_ms
            and self.frame_seq_lost == 0
        )

    def max_gap_near(self, t_s: float, window_s: float = 1.0) -> float:
        """Largest recorded gap that started within `window_s` of stream time `t_s`."""
        near = [g for t, g in self.gaps if abs(t - t_s) <= window_s]
        return max(near, default=0.0)


@dataclass
class StreamRecord:
    """Everything the receiver knows about one stream (one start..stop run of one tab)."""

    index: int
    tab_id: int
    path: Path
    title: str
    started_s: float  # absolute value of the receiver's perf_counter clock
    tracker: GapTracker = field(default_factory=GapTracker)
    ended_s: float | None = None
    stop_reason: str | None = None
    clean_stop: bool = False
    sender_frames_dropped: int = 0
    sender_frames_sent: int | None = None
    sender_samples_sent: int | None = None
    wav: WavWriter | None = None

    def to_dict(self, t0: float = 0.0) -> dict[str, Any]:
        """JSON view; `t0` is the receiver start, so that times are relative to it."""
        t = self.tracker
        return {
            "stream": self.index,
            "tab_id": self.tab_id,
            "title": self.title,
            "file": self.path.name,
            "started_s": round(self.started_s - t0, 3),
            "ended_s": None if self.ended_s is None else round(self.ended_s - t0, 3),
            "stop_reason": self.stop_reason,
            "clean_stop": self.clean_stop,
            "frames": t.frames,
            "samples": t.samples,
            "audio_duration_s": round(t.audio_duration_s, 3),
            "wall_duration_s": round(t.wall_duration_s, 3),
            "max_gap_wall_ms": round(t.max_gap_wall_ms, 2),
            "max_gap_samples_ms": round(t.max_gap_samples_ms, 2),
            "max_gap_sender_ms": round(t.max_gap_sender_ms, 2),
            "drift_ms": round(t.drift_ms, 2),
            "frame_seq_lost": t.frame_seq_lost,
            "sender_frames_dropped": self.sender_frames_dropped,
            "sender_frames_sent": self.sender_frames_sent,
            "sender_samples_sent": self.sender_samples_sent,
            "gaps_over_report_ms": [[round(a, 3), round(b, 1)] for a, b in t.gaps[:50]],
            "gap_ok": t.gap_ok(),
        }


# --- verdicts ----------------------------------------------------------------------------------


def restart_check(streams: list[StreamRecord]) -> bool | None:
    """Clean stop and restart: a tab that streamed twice, with the first run stopped by a message.

    None when no tab was restarted during the session.
    """
    by_tab: dict[int, list[StreamRecord]] = {}
    for s in streams:
        by_tab.setdefault(s.tab_id, []).append(s)
    restarted = [runs for runs in by_tab.values() if len(runs) >= 2]
    if not restarted:
        return None
    for runs in restarted:
        for earlier, later in zip(runs, runs[1:], strict=False):
            if not (earlier.clean_stop and earlier.tracker.frames > 0 and later.tracker.frames > 0):
                return False
    return True


def second_tab_check(streams: list[StreamRecord]) -> bool | None:
    """A second tab joined while the first one kept running as one uninterrupted stream.

    None when no two tabs overlapped.
    """
    verdict: bool | None = None
    for first in streams:
        for second in streams:
            if second.tab_id == first.tab_id or second.started_s <= first.started_s:
                continue
            first_end = first.ended_s if first.ended_s is not None else float("inf")
            if second.started_s >= first_end or first.tracker.first_arrival is None:
                continue
            offset = second.started_s - first.tracker.first_arrival
            ok = (
                first_end > second.started_s
                and first.tracker.frames > 0
                and second.tracker.frames > 0
                and first.tracker.frame_seq_lost == 0
                and first.tracker.max_gap_near(offset) <= GAP_LIMIT_MS
            )
            if ok:
                return True
            verdict = False
    return verdict


def evaluate(streams: list[StreamRecord], audible: bool | None = None) -> dict[str, Any]:
    """Compute the P7 criteria: True = met, False = not met, None = not exercised."""
    if streams:
        no_gap: bool | None = all(s.tracker.gap_ok() for s in streams)
    else:
        no_gap = None
    longest = max((s.tracker.audio_duration_s for s in streams), default=0.0)
    long_run = True if longest >= LONG_RUN_S else None
    criteria = {
        "no_gap_over_100ms": no_gap,
        "long_run_30min": long_run,
        "clean_stop_restart": restart_check(streams),
        "second_tab_without_restart": second_tab_check(streams),
    }
    manual = {"tab_audible": audible}
    values = list(criteria.values()) + [manual["tab_audible"]]
    if any(v is False for v in values):
        overall = "fail"
    elif all(v is True for v in values):
        overall = "pass"
    else:
        overall = "incomplete"
    return {"criteria": criteria, "manual": manual, "overall": overall}


def now_s() -> float:
    return time.perf_counter()
