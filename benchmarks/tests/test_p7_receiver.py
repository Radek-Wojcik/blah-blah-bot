"""End-to-end receiver test with a fake client (skipped when `websockets` is not installed)."""

import asyncio
import json
import struct
import wave

import pytest

pytest.importorskip("websockets")

from websockets.asyncio.client import connect  # noqa: E402
from websockets.exceptions import InvalidStatus  # noqa: E402

from bbb_bench.proto import p7_protocol as p7  # noqa: E402
from bbb_bench.proto import p7_receiver  # noqa: E402

EXT = "c" * 32
ORIGIN = f"chrome-extension://{EXT}"


def hello(token):
    return json.dumps({"type": "hello", "version": 1, "token": token})


def start(tab):
    return json.dumps(
        {"type": "start", "tab_id": tab, "sample_rate": 16000, "channels": 1, "format": "s16le"}
    )


async def scenario(tmp_path):
    receiver = p7_receiver.Receiver(tmp_path / "s", p7.PairingStore("GOODTOKEN"), EXT)
    async with p7_receiver.serve(
        receiver.handler, "127.0.0.1", 0, process_request=receiver.check_origin
    ) as server:
        port = server.sockets[0].getsockname()[1]
        url = f"ws://127.0.0.1:{port}/"

        with pytest.raises(InvalidStatus):  # wrong origin
            async with connect(url, origin="https://evil.example"):
                pass

        async with connect(url, origin=ORIGIN) as bad:  # wrong token
            await bad.send(hello("WRONG"))
            assert json.loads(await bad.recv())["type"] == "error"

        async with connect(url, origin=ORIGIN) as ws:
            await ws.send(hello("GOODTOKEN"))
            assert json.loads(await ws.recv())["type"] == "hello_ok"
            pcm = struct.pack("<320h", *([100] * 320))
            for tab in (1, 2):
                await ws.send(start(tab))
                for seq in range(10):
                    await ws.send(p7.pack_frame(tab, seq, float(seq * 20), pcm))
                    await asyncio.sleep(0.005)
            await ws.send(json.dumps({"type": "stop", "tab_id": 1, "reason": "user_stop"}))
            await ws.send(start(1))  # restart of tab 1
            await ws.send(p7.pack_frame(1, 0, 0.0, pcm))
            await asyncio.sleep(0.05)

        async with connect(url, origin=ORIGIN) as reuse:  # the token was one-time
            await reuse.send(hello("GOODTOKEN"))
            assert json.loads(await reuse.recv())["type"] == "error"
    return receiver


def test_receiver_records_streams_and_rejects_bad_clients(tmp_path):
    receiver = asyncio.run(scenario(tmp_path))
    assert [s.tab_id for s in receiver.streams] == [1, 2, 1]
    first, second, third = receiver.streams
    assert first.clean_stop and first.tracker.frames == 10
    assert not second.clean_stop and second.stop_reason == "connection_closed"
    assert third.tracker.frames == 1 and third.stop_reason == "connection_closed"
    with wave.open(str(first.path), "rb") as wf:
        assert wf.getnframes() == 3200
    assert p7.restart_check(receiver.streams) is True
