import jsonschema

from bbb_bench.proto import p5_llm


def test_transcript_lines_have_monotonic_stamps():
    script = {
        "speakers": ["alex", "kasia"],
        "turns": [{"speaker": "alex", "text": "word " * 280}, {"speaker": "kasia", "text": "ok"}],
    }
    lines, entries = p5_llm.transcript_lines(script)
    assert lines[0].startswith("[00:00] alex:")
    assert lines[1].startswith("[02:00] kasia:")
    assert entries[0]["t_end_ms"] == entries[1]["t_start_ms"]


def test_stamp_ms():
    assert p5_llm._stamp_ms("02:05") == 125_000


def test_schema_accepts_valid_and_rejects_bad_date():
    ok = {
        "items": [
            {
                "type": "task",
                "text": "x",
                "assignee": None,
                "due": "2026-11-20",
                "time": "01:02",
                "quote": "q",
            }
        ]
    }
    jsonschema.validate(ok, p5_llm.EXTRACTION_SCHEMA)
    ok["items"][0]["due"] = "next friday"
    try:
        jsonschema.validate(ok, p5_llm.EXTRACTION_SCHEMA)
    except jsonschema.ValidationError:
        return
    raise AssertionError("bad date accepted")
