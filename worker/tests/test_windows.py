import time

from bbb_worker import windows
from bbb_worker.windows import Line

from .llm_support import context_doc, segment, transcript_doc

MIN = 60_000


def lines(spans: list[tuple[int, int]]) -> list[Line]:
    return [
        Line(s, e, "mic", "user", "You", f"line {i}", segment_id=i)
        for i, (s, e) in enumerate(spans)
    ]


def continuous(minutes: int, step_ms: int = 10_000) -> list[tuple[int, int]]:
    return [(t, t + step_ms - 2_000) for t in range(0, minutes * MIN, step_ms)]


def test_short_transcript_is_one_window():
    parts = windows.split_windows(lines(continuous(5)))
    assert len(parts) == 1
    assert parts[0].span_start_ms == 0
    assert len(parts[0].lines) == 30


def test_empty_transcript_has_no_windows():
    assert windows.split_windows([]) == []


def test_windows_are_cut_near_the_nominal_boundary_with_one_minute_overlap():
    speech = lines(continuous(40))
    parts = windows.split_windows(speech)
    assert len(parts) == 3
    first, second = parts[0], parts[1]
    assert abs(first.lines[-1].start_ms - 15 * MIN) <= 10_000
    cut = first.span_end_ms
    assert second.lines[0].start_ms == cut - MIN
    overlap = {ln.segment_id for ln in first.lines} & {ln.segment_id for ln in second.lines}
    assert len(overlap) == 6  # one minute of 10-second lines


def test_cut_is_at_the_longest_pause_in_the_last_two_minutes():
    spans = [(s, e) for s, e in continuous(30) if not 13 * MIN <= s < 14 * MIN]
    spans = [(s, e) for s, e in spans if not 14 * MIN + 30_000 <= s < 14 * MIN + 45_000]
    # pauses: 13:00-14:00 (60 s) and 14:30-14:45 (15 s) inside the last two minutes of 15:00
    parts = windows.split_windows(lines(spans))
    assert parts[0].lines[-1].end_ms <= 13 * MIN
    assert parts[0].span_end_ms == 14 * MIN


def test_pause_outside_the_search_range_is_ignored():
    spans = [(s, e) for s, e in continuous(30) if not 10 * MIN <= s < 11 * MIN]
    parts = windows.split_windows(lines(spans))
    assert parts[0].span_end_ms >= 15 * MIN - 10_000


def test_every_line_is_in_at_least_one_window():
    spans = continuous(95)
    speech = lines(spans)
    parts = windows.split_windows(speech)
    seen = {ln.segment_id for w in parts for ln in w.lines}
    assert seen == {ln.segment_id for ln in speech}
    assert len(parts) >= 7


def test_screenshots_are_placed_in_their_window_by_time():
    speech = lines(continuous(40))
    shots = [
        Line(5 * MIN, 5 * MIN, "screenshot", "screenshot", "screenshot", "a chart", "screenshot")
    ]
    shots.append(
        Line(30 * MIN, 30 * MIN, "screenshot", "screenshot", "screenshot", "a table", "screenshot")
    )
    parts = windows.split_windows(speech, shots)
    texts = [[ln.text for ln in w.lines if ln.kind == windows.SCREENSHOT] for w in parts]
    assert texts[0] == ["a chart"]
    assert any("a table" in t for t in texts[1:])
    assert "a table" not in texts[0]


def test_render_lines():
    speech = Line(3_725_000, 3_730_000, "mic", "user", "Alex", "Hello there")
    shot = Line(61_000, 61_000, "screenshot", "screenshot", "screenshot", "A slide", "screenshot")
    assert windows.render_line(speech) == "[01:02:05] Alex: Hello there"
    assert windows.render_line(shot) == "[00:01:01] (screenshot) A slide"
    assert windows.parse_clock("01:02:05") == 3_725_000


def test_excluded_segments_are_removed_and_edits_applied():
    doc = transcript_doc(
        [
            segment(0, 0, 1000, "keep this"),
            segment(1, 2000, 3000, "secret salary talk"),
            segment(2, 4000, 5000, "wrong text"),
        ]
    )
    edits = {"segments": {"1": {"excluded": True}, "2": {"text": "corrected text"}}}
    speech = windows.speech_lines(doc, edits)
    assert [ln.text for ln in speech] == ["keep this", "corrected text"]
    assert "secret" not in windows.render_window(windows.split_windows(speech)[0])


def test_screenshot_during_an_excluded_passage_is_dropped():
    doc = transcript_doc([segment(0, 0, 1000, "a"), segment(1, 2000, 3000, "b")])
    edits = {"segments": {"1": {"excluded": True}}}
    ranges = windows.excluded_ranges(doc, edits)
    shots = windows.screenshot_lines(
        {
            "items": [
                {"file": "a.png", "offset_ms": 500, "description": "kept"},
                {"file": "b.png", "offset_ms": 2500, "description": "dropped"},
            ]
        },
        ranges,
    )
    assert [s.text for s in shots] == ["kept"]


def test_speakers_use_labels_of_the_transcript():
    doc = transcript_doc([segment(0, 0, 1000, "hi", speaker="mic:SPEAKER_00")])
    doc["speakers"][1]["label"] = "Alex Kowalski"
    assert windows.speech_lines(doc)[0].label == "Alex Kowalski"
    assert context_doc()["participants"]  # builders stay valid


def test_ten_hour_transcript_is_windowed_fast():
    speech = lines([(t, t + 5_000) for t in range(0, 10 * 3600 * 1000, 6_000)])
    started = time.perf_counter()
    parts = windows.split_windows(speech)
    elapsed = time.perf_counter() - started
    assert elapsed < 2.0
    assert 40 <= len(parts) <= 50
    assert {ln.segment_id for w in parts for ln in w.lines} == {ln.segment_id for ln in speech}
