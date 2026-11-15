"""One transcript from the ASR and diarization outputs of all tracks."""

from __future__ import annotations

from .. import protocol, schema, transcript
from ..manifest import Manifest
from . import read_json, write_json


def run(manifest: Manifest) -> None:
    asr = {}
    diarization = {}
    for track in manifest.tracks:
        asr_path = manifest.work_dir / "asr" / f"{track.name}.json"
        if asr_path.exists():
            asr[track.name] = read_json(asr_path)
        diar_path = manifest.work_dir / "diarize" / f"{track.name}.json"
        if diar_path.exists() and not track.is_microphone:
            doc = read_json(diar_path)
            diarization[track.name] = [
                (t["start_ms"], t["end_ms"], t["speaker"]) for t in doc["turns"]
            ]

    doc = transcript.merge(
        manifest.meeting_id,
        [{"name": t.name, "source": t.source} for t in manifest.tracks],
        asr,
        diarization,
    )
    schema.validate("transcript", doc)
    write_json(manifest.work_dir / "transcript.json", doc)
    protocol.progress("merge", 1.0, f"{len(doc['segments'])} segments")
