"""A fake OpenAI-compatible server and builders for extract and reduce tests. No model is used."""

from __future__ import annotations

import json
import threading
from collections.abc import Callable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Self

from bbb_worker import manifest as manifest_mod

# A handler gets the request body and the 1-based number of the request, and returns the
# content of the answer (a string, or a dict that is sent as JSON).
Handler = Callable[[dict[str, Any], int], Any]


class Truncated(str):
    """An answer cut off at the token limit: sent with finish_reason "length"."""


class FakeLlm:
    """Records every request; answers with whatever the handler returns."""

    def __init__(self, handler: Handler) -> None:
        self.handler = handler
        self.requests: list[dict[str, Any]] = []
        self.headers: list[dict[str, str]] = []
        outer = self

        class _Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                outer.requests.append(body)
                outer.headers.append({k.lower(): v for k, v in self.headers.items()})
                answer = outer.handler(body, len(outer.requests))
                status = 200
                if isinstance(answer, tuple):
                    status, answer = answer
                content = answer if isinstance(answer, str) else json.dumps(answer)
                payload = json.dumps(
                    {
                        "choices": [
                            {
                                "message": {"content": content},
                                "finish_reason": "length"
                                if isinstance(answer, Truncated)
                                else "stop",
                            }
                        ],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 5},
                    }
                ).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args: Any) -> None:
                pass

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self._thread = threading.Thread(
            target=lambda: self._server.serve_forever(poll_interval=0.01), daemon=True
        )

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self._server.server_address[1]}/v1"

    def __enter__(self) -> Self:
        self._thread.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self._server.shutdown()
        self._server.server_close()

    def kinds(self) -> list[str]:
        """Name of the response schema of every request: extraction, summary, sections..."""
        return [r["response_format"]["json_schema"]["name"] for r in self.requests]

    def prompts(self) -> str:
        """All message text received, for assertions about what was sent."""
        return "\n".join(m["content"] for r in self.requests for m in r["messages"])


def by_kind(**handlers: Callable[[dict[str, Any], int], Any]) -> Handler:
    """A handler that dispatches on the response schema name of the request."""

    def handle(body: dict[str, Any], n: int) -> Any:
        return handlers[body["response_format"]["json_schema"]["name"]](body, n)

    return handle


def raw_item(
    text: str,
    quote: str,
    time: str,
    *,
    type: str = "task",
    assignee: str | None = None,
    due: str | None = None,
    section: str | None = None,
    relates_to: str | None = None,
) -> dict[str, Any]:
    stated = "stated" if assignee else "unknown"
    return {
        "type": type,
        "text": text,
        "assignee": assignee,
        "due": due,
        "certainty": {"assignee": stated, "due": "stated" if due else "unknown"},
        "section": section,
        "relates_to": relates_to,
        "time": time,
        "quote": quote,
    }


def segment(i: int, start_ms: int, end_ms: int, text: str, speaker: str = "user") -> dict[str, Any]:
    return {
        "id": i,
        "track": "mic",
        "speaker": speaker,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "text": text,
        "words": [],
    }


def transcript_doc(segments: list[dict[str, Any]], language: str = "en") -> dict[str, Any]:
    return {
        "version": 1,
        "meeting_id": "m1",
        "language": language,
        "speakers": [
            {"id": "user", "label": "You", "kind": "user"},
            {"id": "mic:SPEAKER_00", "label": "Speaker 1", "kind": "voice", "track": "mic"},
        ],
        "segments": segments,
    }


def context_doc(**over: Any) -> dict[str, Any]:
    doc: dict[str, Any] = {
        "version": 1,
        "cycle": "TEAM",
        "name": "Team meetings",
        "meeting_date": "2026-11-16",
        "participants": [{"id": "alex", "name": "Alex Kowalski"}, {"id": "sam", "name": "Sam"}],
        "glossary": [{"term": "Zephyr", "hint": "the code name of the monitor mount"}],
        "instruction": "",
        "output_language": "en",
        "mode": "fixed",
        "template": None,
        "meeting_type": None,
        "sections": [],
        "open_topics": [{"id": "t1", "text": "Supplier choice"}],
        "open_issues": [{"id": "#12", "title": "Mount prototype", "assignee": "alex"}],
        "previous_items": [],
    }
    doc.update(over)
    return doc


def llm_options(url: str, kind: str = "external", **extra: Any) -> dict[str, Any]:
    return {"language": "auto", "llm": {"kind": kind, "base_url": url, "model": "fake", **extra}}


def make_meeting(
    root: Path,
    transcript: dict[str, Any],
    context: dict[str, Any],
    options: dict[str, Any],
    files: dict[str, Any] | None = None,
) -> manifest_mod.Manifest:
    """A meeting folder with `work/` inputs written, and its loaded manifest."""
    meeting = root / "meeting"
    work = meeting / "work"
    work.mkdir(parents=True, exist_ok=True)
    (work / "transcript.json").write_text(json.dumps(transcript), encoding="utf-8")
    (work / "context.json").write_text(json.dumps(context), encoding="utf-8")
    for name, doc in (files or {}).items():
        (work / name).write_text(json.dumps(doc), encoding="utf-8")
    path = root / "manifest.json"
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "meeting_id": "m1",
                "tracks": [],
                "screenshots": [],
                "options": options,
            }
        ),
        encoding="utf-8",
    )
    return manifest_mod.load(path, meeting)


def extraction_for(rules: list[dict[str, Any]]) -> Callable[[dict[str, Any], int], Any]:
    """An extraction answer that reports a rule's item whenever its quote is in the window.

    A rule is `{"quote", "text", "fields": {...}}`; the time is read from the window text.
    """
    import re

    def handle(body: dict[str, Any], n: int) -> Any:
        window = body["messages"][1]["content"].split("Transcript window:\n", 1)[1]
        items = []
        for rule in rules:
            found = re.search(rf"\[(\d\d:\d\d:\d\d)\][^\n]*{re.escape(rule['quote'])}", window)
            if found:
                items.append(
                    raw_item(rule["text"], rule["quote"], found.group(1), **rule.get("fields", {}))
                )
        return {"summary": f"Summary of a window with {len(items)} items.", "items": items}

    return handle


def long_transcript(minutes: int = 35, special: dict[int, str] | None = None) -> dict[str, Any]:
    """A transcript with one 8-second line every 10 seconds; `special` replaces lines by index."""
    segments = []
    for i in range(minutes * 6):
        text = (special or {}).get(i, f"Filler talk number {i} about nothing in particular.")
        segments.append(segment(i, i * 10_000, i * 10_000 + 8_000, text))
    return transcript_doc(segments)
