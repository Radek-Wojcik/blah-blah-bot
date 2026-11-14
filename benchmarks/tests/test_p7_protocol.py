import json
import struct
import wave
from pathlib import Path

import pytest

from bbb_bench.proto import p7_protocol as p7
from bbb_bench.proto.p7_protocol import (
    GapTracker,
    PairingStore,
    ProtocolError,
    StreamRecord,
    WavWriter,
)

EXT_ID = "a" * 32


def feed(tracker: GapTracker, arrivals_ms: list[float], samples: int = 320, start_seq: int = 0):
    for i, t in enumerate(arrivals_ms):
        tracker.update(t / 1000.0, samples, start_seq + i, 1_000_000.0 + t)


def make_stream(index, tab, started, arrivals_ms, *, clean=True, ended=None, seq_skip=None):
    rec = StreamRecord(
        index=index, tab_id=tab, path=Path(f"s{index}.wav"), title="", started_s=started
    )
    for i, t in enumerate(arrivals_ms):
        seq = i + (5 if seq_skip is not None and i >= seq_skip else 0)
        rec.tracker.update(started + t / 1000.0, 320, seq, 1_000_000.0 + t)
    rec.ended_s = ended
    rec.clean_stop = clean
    return rec


# --- frames and control messages ---------------------------------------------------------------


def test_parse_frame_roundtrip():
    pcm = struct.pack("<4h", 1, -2, 3, -4)
    frame = p7.parse_frame(p7.pack_frame(7, 42, 1234.5, pcm))
    assert (frame.tab_id, frame.seq, frame.sent_ms) == (7, 42, 1234.5)
    assert frame.payload == pcm
    assert frame.samples == 4


def test_parse_frame_rejects_short_and_odd():
    with pytest.raises(ProtocolError):
        p7.parse_frame(b"\x00" * 10)
    with pytest.raises(ProtocolError):
        p7.parse_frame(p7.pack_frame(1, 0, 0.0, b"\x00\x01\x02"))


def test_header_is_sixteen_bytes():
    assert p7.HEADER_BYTES == 16


def test_parse_control_hello_start_stop():
    hello = p7.parse_control(json.dumps({"type": "hello", "version": 1, "token": "ABC"}))
    assert hello["token"] == "ABC"
    start = {"type": "start", "tab_id": 5, "sample_rate": 16000, "channels": 1, "format": "s16le"}
    assert p7.parse_control(json.dumps(start))["tab_id"] == 5
    assert (
        p7.parse_control(json.dumps({"type": "stop", "tab_id": 5, "reason": "x"}))["type"] == "stop"
    )


@pytest.mark.parametrize(
    "msg",
    [
        "not json",
        "[]",
        {"type": "nope"},
        {"type": "hello", "version": 2, "token": "x"},
        {"type": "hello", "version": 1},
        {"type": "hello", "version": 1, "token": "x", "session": "y"},
        {"type": "start", "tab_id": 1, "sample_rate": 48000, "channels": 1, "format": "s16le"},
        {"type": "start", "tab_id": 1, "sample_rate": 16000, "channels": 2, "format": "s16le"},
        {"type": "start", "tab_id": 1, "sample_rate": 16000, "channels": 1, "format": "f32"},
        {"type": "start", "tab_id": -1, "sample_rate": 16000, "channels": 1, "format": "s16le"},
        {"type": "stop", "tab_id": True},
    ],
)
def test_parse_control_rejects_invalid(msg):
    text = msg if isinstance(msg, str) else json.dumps(msg)
    with pytest.raises(ProtocolError):
        p7.parse_control(text)


# --- origin and pairing ------------------------------------------------------------------------


def test_origin_check():
    assert p7.origin_allowed(f"chrome-extension://{EXT_ID}", EXT_ID)
    assert not p7.origin_allowed(f"chrome-extension://{'b' * 32}", EXT_ID)
    assert not p7.origin_allowed("https://example.com", EXT_ID)
    assert not p7.origin_allowed(f"chrome-extension://{EXT_ID}.evil.com", EXT_ID)
    assert not p7.origin_allowed(None, EXT_ID)
    assert not p7.origin_allowed("", None)
    assert p7.origin_allowed(f"chrome-extension://{'b' * 32}", None)


def test_token_is_one_time_and_session_resumes():
    store = PairingStore("TOKEN123")
    assert store.claim_token("WRONG") is None
    session = store.claim_token("TOKEN123")
    assert session
    assert store.claim_token("TOKEN123") is None
    assert store.check_session(session)
    assert not store.check_session("other")


def test_new_token_shape():
    token = p7.new_token()
    assert len(token) == 8 and token.isalnum()


# --- WAV ---------------------------------------------------------------------------------------


def test_wav_header_fields():
    header = p7.wav_header(32000)
    assert len(header) == 44
    assert header[:4] == b"RIFF" and header[8:12] == b"WAVE"
    (riff_size,) = struct.unpack_from("<I", header, 4)
    assert riff_size == 36 + 32000
    channels, rate, byte_rate, block, bits = struct.unpack_from("<HIIHH", header, 22)
    assert (channels, rate, byte_rate, block, bits) == (1, 16000, 32000, 2, 16)


def test_wav_writer_produces_readable_file(tmp_path):
    path = tmp_path / "a.wav"
    writer = WavWriter(path)
    chunk = struct.pack("<320h", *range(320))
    for _ in range(5):
        writer.write(chunk)
    writer.flush_header()  # header already valid before close
    with wave.open(str(path), "rb") as wf:
        assert wf.getnframes() == 1600
    writer.write(chunk)
    writer.close()
    with wave.open(str(path), "rb") as wf:
        assert (wf.getnchannels(), wf.getsampwidth(), wf.getframerate()) == (1, 2, 16000)
        assert wf.getnframes() == 1920
        assert wf.readframes(3) == struct.pack("<3h", 0, 1, 2)


# --- gap metrics -------------------------------------------------------------------------------


def test_steady_stream_has_no_gap():
    tracker = GapTracker()
    feed(tracker, [i * 20.0 for i in range(500)])
    assert tracker.frames == 500
    assert tracker.audio_duration_s == pytest.approx(10.0)
    assert tracker.max_gap_wall_ms == pytest.approx(20.0)
    assert tracker.max_gap_samples_ms == pytest.approx(0.0, abs=1e-6)
    assert abs(tracker.drift_ms) < 1e-6
    assert tracker.gap_ok()


def test_stall_is_seen_by_both_clocks_and_burst_does_not_hide_it():
    tracker = GapTracker()
    arrivals = [i * 20.0 for i in range(10)]
    stall_end = arrivals[-1] + 250.0
    # After the stall the backlog arrives in a burst: five frames 1 ms apart.
    arrivals += [stall_end + i for i in range(5)]
    feed(tracker, arrivals)
    assert tracker.max_gap_wall_ms == pytest.approx(250.0)
    assert tracker.max_gap_samples_ms == pytest.approx(230.0)
    assert not tracker.gap_ok()
    assert tracker.max_gap_near(arrivals[9] / 1000.0) == pytest.approx(250.0)
    assert tracker.max_gap_near(100.0) == 0.0


def test_gap_at_the_limit_passes():
    tracker = GapTracker()
    feed(tracker, [0.0, 20.0, 120.0, 140.0])
    assert tracker.max_gap_wall_ms == pytest.approx(100.0)
    assert tracker.gap_ok()


def test_sequence_loss_fails_gap_ok():
    tracker = GapTracker()
    tracker.update(0.00, 320, 0, 0.0)
    tracker.update(0.02, 320, 1, 20.0)
    tracker.update(0.08, 320, 4, 80.0)  # frames 2 and 3 never arrived
    assert tracker.frame_seq_lost == 2
    assert not tracker.gap_ok()


def test_slow_stream_reports_drift():
    tracker = GapTracker()
    feed(tracker, [i * 21.0 for i in range(1001)])  # audio arrives 5% slower than real time
    assert tracker.drift_ms == pytest.approx(1000 * 1.0, rel=0.01)
    assert tracker.max_gap_sender_ms == pytest.approx(21.0)


def test_empty_tracker_is_not_ok():
    assert not GapTracker().gap_ok()


# --- verdicts ----------------------------------------------------------------------------------


STEADY = [i * 20.0 for i in range(200)]  # 4 s


def test_restart_check():
    assert p7.restart_check([make_stream(1, 1, 0.0, STEADY)]) is None
    first = make_stream(1, 1, 0.0, STEADY, clean=True, ended=4.0)
    second = make_stream(2, 1, 5.0, STEADY, clean=True, ended=9.0)
    assert p7.restart_check([first, second]) is True
    unclean = make_stream(1, 1, 0.0, STEADY, clean=False, ended=4.0)
    assert p7.restart_check([unclean, second]) is False


def test_second_tab_check_passes_when_first_stream_continues():
    first = make_stream(1, 1, 0.0, STEADY, ended=10.0)
    second = make_stream(2, 2, 2.0, STEADY, ended=10.0)
    assert p7.second_tab_check([first, second]) is True


def test_second_tab_check_fails_if_first_stalls_when_second_joins():
    stalled = [i * 20.0 for i in range(100)] + [1980.0 + 400.0 + i * 20.0 for i in range(50)]
    first = make_stream(1, 1, 0.0, stalled, ended=10.0)
    second = make_stream(2, 2, 2.0, STEADY, ended=10.0)
    assert p7.second_tab_check([first, second]) is False


def test_second_tab_check_not_exercised_without_overlap():
    first = make_stream(1, 1, 0.0, STEADY, ended=4.0)
    second = make_stream(2, 2, 6.0, STEADY, ended=10.0)
    assert p7.second_tab_check([first, second]) is None
    assert p7.second_tab_check([first]) is None


def test_evaluate_overall_states():
    first = make_stream(1, 1, 0.0, STEADY, ended=10.0)
    second = make_stream(2, 2, 2.0, STEADY, ended=10.0)
    result = p7.evaluate([first, second])
    assert result["criteria"]["no_gap_over_100ms"] is True
    assert result["criteria"]["second_tab_without_restart"] is True
    assert result["criteria"]["long_run_30min"] is None
    assert result["manual"] == {"tab_audible": None}
    assert result["overall"] == "incomplete"
    assert p7.evaluate([first, second], audible=False)["overall"] == "fail"
    assert p7.evaluate([])["criteria"]["no_gap_over_100ms"] is None


def test_evaluate_fails_on_gap():
    bad = make_stream(1, 1, 0.0, [0.0, 20.0, 400.0], ended=1.0)
    result = p7.evaluate([bad], audible=True)
    assert result["criteria"]["no_gap_over_100ms"] is False
    assert result["overall"] == "fail"


def test_evaluate_pass_requires_everything():
    long_arrivals = [i * 20.0 for i in range(90_000)]  # 30 min of 20 ms frames
    a = make_stream(1, 1, 0.0, long_arrivals, ended=1800.0)
    a_restart = make_stream(2, 1, 1801.0, STEADY, ended=1806.0)
    b = make_stream(3, 2, 900.0, STEADY, ended=1700.0)
    result = p7.evaluate([a, a_restart, b], audible=True)
    assert result["criteria"] == {
        "no_gap_over_100ms": True,
        "long_run_30min": True,
        "clean_stop_restart": True,
        "second_tab_without_restart": True,
    }
    assert result["overall"] == "pass"


def test_stream_record_to_dict_is_json_serializable():
    rec = make_stream(1, 3, 10.0, STEADY, ended=14.0)
    doc = rec.to_dict(t0=10.0)
    assert doc["started_s"] == 0.0 and doc["ended_s"] == 4.0
    assert doc["frames"] == 200
    json.dumps(doc)
