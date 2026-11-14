"""P7 receiver: local WebSocket server for the Chrome tab-capture extension.

Run (prints a one-time pairing token to type into the extension popup):

    uv run --group p7 python -m bbb_bench.proto.p7_receiver

Each stream (one start..stop run of one tab) is written to `benchmarks/data/p7/<name>/*.wav`.
On exit (Ctrl+C or `--duration`) the P7 metrics go to `benchmarks/results/p7-chrome-<name>.json`.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import time
from datetime import datetime
from http import HTTPStatus
from pathlib import Path
from typing import Any

from websockets.asyncio.server import ServerConnection, serve
from websockets.datastructures import Headers
from websockets.exceptions import ConnectionClosed
from websockets.http11 import Request, Response

from bbb_bench import harness
from bbb_bench.proto.p7_protocol import (
    DEFAULT_EXTENSION_ID,
    GAP_LIMIT_MS,
    PairingStore,
    ProtocolError,
    StreamRecord,
    WavWriter,
    evaluate,
    new_token,
    now_s,
    origin_allowed,
    parse_control,
    parse_frame,
)

DATA_DIR = Path(__file__).resolve().parents[3] / "data" / "p7"
HELLO_TIMEOUT_S = 10.0
MAX_MESSAGE_BYTES = 1 << 20


class Receiver:
    def __init__(self, out_dir: Path, pairing: PairingStore, extension_id: str | None):
        self.out_dir = out_dir
        self.pairing = pairing
        self.extension_id = extension_id
        self.t0 = now_s()
        self.streams: list[StreamRecord] = []
        self.active: dict[int, StreamRecord] = {}
        self.connection_open = False
        self.connections_accepted = 0
        out_dir.mkdir(parents=True, exist_ok=True)

    # --- HTTP upgrade check ------------------------------------------------------------------

    def check_origin(self, connection: ServerConnection, request: Request) -> Response | None:
        origin = request.headers.get("Origin")
        if not origin_allowed(origin, self.extension_id):
            print(f"rejected connection with origin {origin!r}")
            return Response(
                HTTPStatus.FORBIDDEN.value, "Forbidden", Headers(), b"origin not allowed\n"
            )
        return None

    # --- stream bookkeeping ------------------------------------------------------------------

    def _start_stream(self, msg: dict[str, Any]) -> None:
        tab_id = msg["tab_id"]
        previous = self.active.get(tab_id)
        if previous is not None:
            self._finish(previous, "restarted_without_stop", clean=False)
        index = len(self.streams) + 1
        path = self.out_dir / f"tab{tab_id}-stream{index}.wav"
        rec = StreamRecord(
            index=index,
            tab_id=tab_id,
            path=path,
            title=str(msg.get("title", ""))[:200],
            started_s=now_s(),
            wav=WavWriter(path),
        )
        self.streams.append(rec)
        self.active[tab_id] = rec
        print(f"stream {index}: tab {tab_id} started -> {path.name}")

    def _finish(self, rec: StreamRecord, reason: str, clean: bool) -> None:
        if rec.wav is not None:
            rec.wav.close()
        rec.ended_s = now_s()
        rec.stop_reason = reason
        rec.clean_stop = clean
        if self.active.get(rec.tab_id) is rec:
            del self.active[rec.tab_id]
        t = rec.tracker
        print(
            f"stream {rec.index}: tab {rec.tab_id} ended ({reason}); {t.frames} frames, "
            f"{t.audio_duration_s:.1f} s, max gap {t.max_gap_wall_ms:.1f} ms"
        )

    def _handle_control(self, msg: dict[str, Any]) -> None:
        if msg["type"] == "start":
            self._start_stream(msg)
        elif msg["type"] == "stop":
            rec = self.active.get(msg["tab_id"])
            if rec is None:
                raise ProtocolError(f"stop for tab {msg['tab_id']} without an active stream")
            rec.sender_frames_dropped = int(msg.get("frames_dropped") or 0)
            rec.sender_frames_sent = msg.get("frames_sent")
            rec.sender_samples_sent = msg.get("samples_sent")
            self._finish(rec, str(msg.get("reason") or "stop"), clean=True)
        else:
            raise ProtocolError(f"unexpected {msg['type']!r} after pairing")

    def _handle_frame(self, data: bytes, arrival_s: float) -> None:
        frame = parse_frame(data)
        rec = self.active.get(frame.tab_id)
        if rec is None:
            raise ProtocolError(f"frame for tab {frame.tab_id} without an active stream")
        assert rec.wav is not None
        rec.wav.write(frame.payload)
        rec.tracker.update(arrival_s, frame.samples, frame.seq, frame.sent_ms)

    def flush_headers(self) -> None:
        for rec in self.active.values():
            if rec.wav is not None:
                rec.wav.flush_header()

    # --- connection handler ------------------------------------------------------------------

    async def _authenticate(self, connection: ServerConnection) -> str | None:
        raw = await asyncio.wait_for(connection.recv(), HELLO_TIMEOUT_S)
        if not isinstance(raw, str):
            raise ProtocolError("first message must be a text hello")
        hello = parse_control(raw)
        if hello["type"] != "hello":
            raise ProtocolError("first message must be hello")
        if "token" in hello:
            session = self.pairing.claim_token(hello["token"])
        elif self.pairing.check_session(hello["session"]):
            session = hello["session"]
        else:
            session = None
        if session is None:
            return None
        print(f"paired; client: {hello.get('user_agent', 'unknown')}")
        return session

    async def handler(self, connection: ServerConnection) -> None:
        if self.connection_open:
            await connection.send(json.dumps({"type": "error", "reason": "already connected"}))
            await connection.close(1008, "already connected")
            return
        self.connection_open = True
        try:
            try:
                session = await self._authenticate(connection)
            except (ProtocolError, TimeoutError, ConnectionClosed) as exc:
                print(f"hello failed: {exc!r}")
                await connection.close(1008, "bad hello")
                return
            if session is None:
                await connection.send(
                    json.dumps({"type": "error", "reason": "invalid or used pairing token"})
                )
                await connection.close(1008, "unauthorized")
                return
            self.connections_accepted += 1
            await connection.send(json.dumps({"type": "hello_ok", "session": session}))
            async for message in connection:
                arrival = now_s()  # taken before any parsing, to keep timing honest
                try:
                    if isinstance(message, str):
                        self._handle_control(parse_control(message))
                    else:
                        self._handle_frame(message, arrival)
                except ProtocolError as exc:
                    print(f"protocol error: {exc}")
                    await connection.send(json.dumps({"type": "error", "reason": str(exc)}))
                    await connection.close(1002, "protocol error")
                    return
        except ConnectionClosed:
            pass
        finally:
            for rec in list(self.active.values()):
                self._finish(rec, "connection_closed", clean=False)
            self.connection_open = False

    # --- reporting ---------------------------------------------------------------------------

    def summary_line(self) -> str:
        parts = [
            f"tab {r.tab_id}: {r.tracker.audio_duration_s:6.1f} s, "
            f"max gap {r.tracker.max_gap_wall_ms:5.1f} ms"
            for r in self.active.values()
        ]
        return "; ".join(parts) if parts else "no active streams"

    def result_payload(self, audible: bool | None, started_at: str) -> dict[str, Any]:
        verdict = evaluate(self.streams, audible)
        return {
            "status": "ok",
            "started_at": started_at,
            "gap_limit_ms": GAP_LIMIT_MS,
            "connections_accepted": self.connections_accepted,
            "streams": [s.to_dict(self.t0) for s in self.streams],
            **verdict,
            "notes": (
                "criteria: true = met, false = not met, null = not exercised in this session; "
                "manual.tab_audible is a listening test set with --audible."
            ),
        }


async def _periodic(receiver: Receiver, quiet: bool) -> None:
    ticks = 0
    while True:
        await asyncio.sleep(5)
        receiver.flush_headers()
        ticks += 1
        if not quiet and ticks % 6 == 0:
            print(f"[{time.strftime('%H:%M:%S')}] {receiver.summary_line()}")


async def run(args: argparse.Namespace) -> Path:
    name = args.name or datetime.now().strftime("%Y%m%d-%H%M%S")
    token = args.token or new_token()
    extension_id = None if args.extension_id == "any" else args.extension_id
    receiver = Receiver(args.out_dir / name, PairingStore(token), extension_id)
    started_at = datetime.now().astimezone().isoformat(timespec="seconds")
    audible = {"yes": True, "no": False, None: None}[args.audible]

    async with serve(
        receiver.handler,
        "127.0.0.1",
        args.port,
        process_request=receiver.check_origin,
        max_size=MAX_MESSAGE_BYTES,
        ping_interval=None,  # keep-alive pings could disturb timing; the OS notices dead peers
    ):
        print(
            f"P7 receiver listening on ws://127.0.0.1:{args.port}/ (extension id: {extension_id})"
        )
        print(f"Pairing token (single use): {token}")
        print(f"Writing WAV files to {receiver.out_dir}")
        print("Press Ctrl+C to stop and write the result file.")
        ticker = asyncio.create_task(_periodic(receiver, args.quiet))
        try:
            if args.duration:
                await asyncio.sleep(args.duration)
            else:
                await asyncio.Event().wait()
        except asyncio.CancelledError:
            pass
        finally:
            ticker.cancel()
    # Leaving `serve` closed every connection; handlers have finalized their streams.
    for rec in list(receiver.active.values()):
        receiver._finish(rec, "receiver_shutdown", clean=False)
    payload = receiver.result_payload(audible, started_at)
    if args.no_result:
        print(f"result (not written): {payload['overall']}")
        return receiver.out_dir
    path = harness.write_result("p7-chrome", name, payload)
    print(f"result: {payload['overall']} -> {path}")
    print(json.dumps({"criteria": payload["criteria"], "manual": payload["manual"]}, indent=2))
    return path


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--port", type=int, default=17555)
    parser.add_argument("--name", help="session name (default: timestamp)")
    parser.add_argument("--token", help="pairing token (default: random)")
    parser.add_argument(
        "--extension-id",
        default=DEFAULT_EXTENSION_ID,
        help="allowed extension id, or 'any' to accept any chrome-extension:// origin",
    )
    parser.add_argument("--duration", type=float, help="stop automatically after N seconds")
    parser.add_argument(
        "--audible",
        choices=["yes", "no"],
        help="manual verdict: was the captured tab audible during the test?",
    )
    parser.add_argument("--out-dir", type=Path, default=DATA_DIR)
    parser.add_argument("--no-result", action="store_true", help="do not write a result file")
    parser.add_argument("--quiet", action="store_true", help="no periodic status line")
    args = parser.parse_args(argv)
    try:
        asyncio.run(run(args))
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
