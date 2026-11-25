# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=1.26"]
# ///
"""Hardware tests for the P1 audio recorder.

Starts test players (ffplay, a child process behind cmd, an isolated Chrome instance), records
them with the recorder binary and checks the recordings with spectral and cross-correlation
analysis. Results accumulate in ``benchmarks/results/p1-audio-<windows build>.json``.

Run through the recorder: ``p1-audio selftest [scenario ...]``.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import math
import os
import platform
import shutil
import subprocess
import sys
import time
import wave
from datetime import UTC, datetime
from pathlib import Path

import numpy as np

RATE = 16_000
HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
DEFAULT_RESULTS = BENCH / "results"
DEFAULT_DATA = BENCH / "data"
NO_WINDOW = 0x08000000
CHROME = Path(os.environ.get("P1_CHROME", r"C:\Program Files\Google\Chrome\Application\chrome.exe"))

TONES = {"a": 440, "b": 1000, "c": 2000, "outside": 3000}
ISOLATION_LIMIT_DB = -40.0
ALIGNMENT_LIMIT_MS = 50.0
CPU_LIMIT_PERCENT = 3.0
MEMORY_LIMIT_MB = 200.0
KILL_LOSS_LIMIT_S = 60.0


# ---------------------------------------------------------------- process helpers


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", **kw)


def ps_json(script: str):
    out = run(["powershell", "-NoProfile", "-Command", script]).stdout.strip()
    if not out:
        return []
    data = json.loads(out)
    return data if isinstance(data, list) else [data]


def process_table() -> list[dict]:
    return ps_json(
        "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,"
        "CommandLine | ConvertTo-Json -Compress"
    )


def descendants(root: int, table: list[dict]) -> set[int]:
    found = {root}
    changed = True
    while changed:
        changed = False
        for p in table:
            if p["ParentProcessId"] in found and p["ProcessId"] not in found:
                found.add(p["ProcessId"])
                changed = True
    return found


class Recorder:
    """Wrapper around the recorder binary."""

    def __init__(self, exe: Path):
        self.exe = exe

    def sessions(self) -> list[dict]:
        out = run([str(self.exe), "sessions", "--json"])
        return json.loads(out.stdout) if out.returncode == 0 and out.stdout.strip() else []

    def wait_session(self, pid: int, timeout: float = 15.0, active: bool = True) -> dict | None:
        end = time.time() + timeout
        while time.time() < end:
            for s in self.sessions():
                if s["pid"] == pid and (not active or s["state"] == "Active"):
                    # Windows remembers a muted level per application; test players must be audible
                    # to the loopback, which records after the session volume.
                    if s["process"].lower() == "ffplay.exe" and (s["muted"] or s["volume"] < 1.0):
                        run([str(self.exe), "session-volume", "--pid", str(pid), "--level", "1.0"])
                    return s
            time.sleep(0.4)
        return None

    def record(self, out: Path, duration: float, args: list[str], segment_s: int = 60) -> dict:
        if out.exists():
            shutil.rmtree(out)
        cmd = [str(self.exe), "record", "--out", str(out), "--duration", str(duration)]
        cmd += [*args, "--segment-s", str(segment_s)]
        res = run(cmd)
        summary_path = out / "summary.json"
        summary = json.loads(summary_path.read_text("utf-8")) if summary_path.exists() else {}
        return {"returncode": res.returncode, "stderr": res.stderr.strip(), "summary": summary}


class Player:
    """A sound source in its own process (ffplay, or the recorder's own `play` helper)."""

    def __init__(self, cmd: list[str], via_cmd: bool = False):
        if via_cmd:
            cmd = ["cmd", "/c", *cmd]
        self.proc = subprocess.Popen(
            cmd, creationflags=NO_WINDOW, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        self.pid = self.proc.pid

    def stop(self) -> None:
        with contextlib.suppress(Exception):
            subprocess.run(
                ["taskkill", "/PID", str(self.pid), "/T", "/F"], capture_output=True, check=False
            )


class SilentProcess(Player):
    """A process without audio. Process loopback of it still yields a continuous (silent)
    stream, which is enough to measure continuity, CPU, memory and clock drift."""

    def __init__(self, seconds: int):
        super().__init__(["ping", "-n", str(seconds), "127.0.0.1"])


def make_tone(path: Path, freq: int, seconds: int = 30) -> None:
    cmd = [
        "ffmpeg",
        "-y",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        f"sine=frequency={freq}:duration={seconds}:sample_rate=48000",
        "-af",
        "volume=0.25",
        str(path),
    ]
    subprocess.run(cmd, check=True)


def click_template(rate: int) -> np.ndarray:
    """A 10 ms Hann-windowed 2 kHz burst."""
    n = int(rate * 0.010)
    t = np.arange(n) / rate
    return 0.6 * np.hanning(n) * np.sin(2 * np.pi * 2000 * t)


def make_click_file(path: Path, seconds: int = 45, period: float = 1.0, first: float = 0.5) -> None:
    rate = 48_000
    x = np.zeros(rate * seconds)
    h = click_template(rate)
    t = first
    while t + 0.02 < seconds:
        i = int(t * rate)
        x[i : i + len(h)] += h
        t += period
    pcm = (x * 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(pcm.tobytes())


# ---------------------------------------------------------------- analysis


def read_track(session_dir: Path, track: str) -> tuple[float, np.ndarray]:
    """Return (offset_ms, samples as float in -1..1) from the journal and segment files."""
    offset = 0.0
    segments = []
    for line in (session_dir / "journal.jsonl").read_text("utf-8").splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if e.get("track") != track:
            continue
        if e["event"] == "track_start":
            offset = e["offset_ms"]
        elif e["event"] == "segment":
            segments.append(e)
    parts = []
    for s in sorted(segments, key=lambda s: s["segment"]):
        with wave.open(str(session_dir / s["file"]), "rb") as w:
            parts.append(np.frombuffer(w.readframes(w.getnframes()), dtype="<i2"))
    if not parts:
        return offset, np.zeros(0)
    return offset, np.concatenate(parts).astype(np.float64) / 32768.0


def band_power(x: np.ndarray, freq: float, bw: float = 15.0, skip_s: float = 1.0) -> float:
    seg = x[int(skip_s * RATE) :]
    if len(seg) < RATE:
        return 0.0
    spec = np.abs(np.fft.rfft(seg * np.hanning(len(seg)))) ** 2
    f = np.fft.rfftfreq(len(seg), 1 / RATE)
    return float(spec[(f >= freq - bw) & (f <= freq + bw)].sum())


def rel_db(power: float, ref: float) -> float:
    if ref <= 0:
        return float("nan")
    return max(-150.0, 10 * math.log10(max(power, 1e-30) / ref))


def tone_report(x: np.ndarray, own: int | None, freqs: dict[str, int]) -> dict:
    """Level of every test tone in a track, relative to the track's own tone."""
    powers = {name: band_power(x, f) for name, f in freqs.items()}
    own_name = next((n for n, f in freqs.items() if f == own), None)
    ref = powers[own_name] if own_name else max(powers.values())
    levels = {n: rel_db(p, ref) for n, p in powers.items()}
    rms = float(np.sqrt(np.mean(x**2))) if len(x) else 0.0
    others = [v for n, v in levels.items() if n != own_name]
    return {
        "levels_db_vs_own": {n: round(v, 1) for n, v in levels.items()},
        "own_present": own_name is not None and ref > 0 and rms > 1e-4,
        "worst_other_db": round(max(others), 1) if others else None,
        "rms_dbfs": round(20 * math.log10(max(rms, 1e-9)), 1),
    }


def analytic_env(x: np.ndarray) -> np.ndarray:
    n = len(x)
    spec = np.fft.fft(x)
    h = np.zeros(n)
    h[0] = 1
    if n % 2 == 0:
        h[n // 2] = 1
        h[1 : n // 2] = 2
    else:
        h[1 : (n + 1) // 2] = 2
    return np.abs(np.fft.ifft(spec * h))


def click_times_ms(offset_ms: float, x: np.ndarray, threshold: float = 0.4) -> np.ndarray:
    """Session-clock times of clicks found by matched filtering."""
    h = click_template(RATE)
    n = len(x) + len(h)
    corr = np.fft.irfft(np.fft.rfft(x, n) * np.conj(np.fft.rfft(h, n)), n)[: len(x)]
    env = analytic_env(corr)
    if env.max() <= 0:
        return np.zeros(0)
    limit = threshold * env.max()
    min_gap = int(0.4 * RATE)
    peaks: list[int] = []
    order = np.argsort(env)[::-1]
    for i in order:
        if env[i] < limit:
            break
        if all(abs(i - p) >= min_gap for p in peaks):
            peaks.append(int(i))
    peaks.sort()
    # The correlation peak sits at the start of the burst; add half a burst to its centre.
    centre = len(h) / 2
    return offset_ms + (np.array(peaks) + centre) / RATE * 1000.0


def pair_clicks(
    ref_ms: np.ndarray, other_ms: np.ndarray, ambiguous_ms: np.ndarray, window_ms: float = 80.0
) -> list[float]:
    """Differences (ref - other) for clicks of ref that have exactly one counterpart."""
    deltas = []
    for a in ref_ms:
        if len(ambiguous_ms) and np.min(np.abs(ambiguous_ms - a)) < 2 * window_ms:
            continue
        if len(other_ms) == 0:
            continue
        j = int(np.argmin(np.abs(other_ms - a)))
        if abs(other_ms[j] - a) <= window_ms:
            deltas.append(float(a - other_ms[j]))
    return deltas


# ---------------------------------------------------------------- scenarios


class NeedsConsent(Exception):
    """The scenario would play sound on the user's default output device."""


class Context:
    def __init__(
        self, rec: Recorder, data: Path, quick: bool, audible: bool, output_device: str | None
    ):
        self.rec = rec
        self.data = data
        self.quick = quick
        self.audible = audible
        self.output_device = output_device
        self.keep_mic_audio = False
        self.mic_device: str | None = None
        self.no_drift_correction = False
        # drift: microphone tuning experiment and a run length override.
        self.mic_buffer_ms: int | None = None
        self.mic_mmcss = False
        self.minutes: float | None = None
        self.work = data / "p1-selftest"
        self.work.mkdir(parents=True, exist_ok=True)
        self.tones: dict[int, Path] = {}

    def player(
        self,
        freq: int | None = None,
        click: bool = False,
        via_cmd: bool = False,
        limit_s: int = 600,
        volume: int = 40,
    ) -> Player:
        """Start a sound source. Without --output-device the sound would go to the default
        device, which needs an explicit --audible."""
        if self.output_device:
            cmd = [
                str(self.rec.exe),
                "play",
                "--device",
                self.output_device,
                "--seconds",
                str(limit_s),
            ]
            cmd += ["--click"] if click else ["--tone", str(freq)]
            return Player(cmd, via_cmd)
        if not self.audible:
            raise NeedsConsent(
                "plays sound on the default output device; use --output-device NAME or --audible"
            )
        if click:
            path = self.work / "clicks.wav"
            make_click_file(path)
        else:
            path = self.tone_file(freq)
        cmd = [
            "ffplay",
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "error",
            "-volume",
            str(volume),
            "-loop",
            "1" if click else "0",
            "-t",
            str(limit_s),
            str(path),
        ]
        return Player(cmd, via_cmd)

    def tone_file(self, freq: int) -> Path:
        if freq not in self.tones:
            p = self.work / f"tone-{freq}.wav"
            make_tone(p, freq)
            self.tones[freq] = p
        return self.tones[freq]


def summarize_run(run_result: dict) -> dict:
    s = run_result["summary"]
    if not s:
        return {"error": run_result["stderr"] or "no summary"}
    return {
        "elapsed_s": round(s["elapsed_s"], 2),
        "dropped_buffers": s["totals"]["dropped_buffers"],
        "failed_tracks": s["totals"]["failed_tracks"],
        "cpu_avg_percent_total": round(s["cpu"]["avg_percent_total"], 4),
        "peak_working_set_mb": round(s["memory"]["peak_working_set_mb"], 2),
        "silence_fill_events": s["totals"]["silence_fill_events"],
    }


def scenario_version(ctx: Context) -> dict:
    out = run([str(ctx.rec.exe), "probe"])
    lines = [ln for ln in out.stdout.splitlines() if ln.strip()]
    tries = [ln for ln in lines if ln.startswith("include_tree=")]
    return {
        "windows_version": platform.version(),
        "windows_release": platform.release(),
        "probe_output": lines,
        "activation_ok_all": bool(tries) and all(t.endswith(": ok") for t in tries),
        "pass": bool(tries) and all(t.endswith(": ok") for t in tries),
    }


def scenario_isolation(ctx: Context) -> dict:
    dur = 10 if ctx.quick else 14
    players = {n: ctx.player(f) for n, f in TONES.items()}
    try:
        for n, p in players.items():
            if not ctx.rec.wait_session(p.pid):
                return {"pass": False, "error": f"no active session for tone {n}"}
        time.sleep(1.5)
        args = ["--system"]
        for n in ("a", "b", "c"):
            args += ["--pid", str(players[n].pid)]
        res = ctx.rec.record(ctx.work / "isolation", dur, args)
    finally:
        for p in players.values():
            p.stop()
    out = {"run": summarize_run(res), "tracks": {}}
    ok = True
    for n in ("a", "b", "c"):
        _, x = read_track(ctx.work / "isolation", f"pid-{players[n].pid}")
        rep = tone_report(x, TONES[n], TONES)
        rep["pass"] = bool(
            rep["own_present"]
            and rep["worst_other_db"] is not None
            and rep["worst_other_db"] <= ISOLATION_LIMIT_DB
        )
        out["tracks"][n] = rep
        ok &= rep["pass"]
    _, sysx = read_track(ctx.work / "isolation", "system")
    sysrep = tone_report(sysx, None, TONES)
    out["system_reference_levels_db_vs_loudest"] = sysrep["levels_db_vs_own"]
    # The reference track must contain every tone, otherwise the test proves nothing.
    out["reference_has_all_tones"] = all(v > -30 for v in sysrep["levels_db_vs_own"].values())
    out["limit_db"] = ISOLATION_LIMIT_DB
    out["pass"] = bool(ok and not out["run"].get("failed_tracks"))
    return out


def scenario_tree(ctx: Context) -> dict:
    dur = 10 if ctx.quick else 12
    child = ctx.player(TONES["b"], via_cmd=True)
    outside = ctx.player(TONES["outside"])
    try:
        time.sleep(2.0)
        table = process_table()
        kids = [p for p in table if p["ParentProcessId"] == child.pid and p["Name"] == "ffplay.exe"]
        if not kids:
            return {"pass": False, "error": "no ffplay child under cmd"}
        kid_pid = kids[0]["ProcessId"]
        sess = ctx.rec.wait_session(kid_pid)
        if not sess or not ctx.rec.wait_session(outside.pid):
            return {"pass": False, "error": "players have no active session"}
        res = ctx.rec.record(
            ctx.work / "tree",
            dur,
            ["--pid", f"{child.pid}", "--pid", f"{child.pid}:exclude", "--system"],
        )
        res_kid_only = ctx.rec.record(ctx.work / "tree-kid", 6, ["--pid", f"{kid_pid}"])
    finally:
        child.stop()
        outside.stop()
    freqs = {"child_tone": TONES["b"], "outside_tone": TONES["outside"]}
    inc = tone_report(read_track(ctx.work / "tree", f"pid-{child.pid}")[1], TONES["b"], freqs)
    exc = tone_report(
        read_track(ctx.work / "tree", f"pid-{child.pid}-excl")[1], TONES["outside"], freqs
    )
    kid = tone_report(read_track(ctx.work / "tree-kid", f"pid-{kid_pid}")[1], TONES["b"], freqs)
    inc["pass"] = bool(inc["own_present"] and inc["worst_other_db"] <= ISOLATION_LIMIT_DB)
    exc["pass"] = bool(exc["own_present"] and exc["worst_other_db"] <= ISOLATION_LIMIT_DB)
    kid["pass"] = bool(kid["own_present"] and kid["worst_other_db"] <= ISOLATION_LIMIT_DB)
    return {
        "cmd_pid": child.pid,
        "ffplay_child_pid": kid_pid,
        "session_owner_of_child": {
            "pid": sess["pid"],
            "parent_pid": sess["parent_pid"],
            "process": sess["process"],
        },
        "include_tree_on_parent_cmd": {
            "description": "records the ffplay child, not the outside tone",
            **inc,
        },
        "exclude_mode_on_parent_cmd": {
            "description": "records everything except the tree: "
            "outside tone present, child tone absent",
            **exc,
        },
        "child_pid_direct": kid,
        "run": summarize_run(res),
        "run_kid": summarize_run(res_kid_only),
        "pass": bool(inc["pass"] and exc["pass"] and kid["pass"]),
    }


CHROME_PAGE = """<!doctype html>
<meta charset="utf-8"><title>p1 tone</title>
<script>
const ctx = new AudioContext();
const osc = ctx.createOscillator();
const gain = ctx.createGain();
osc.frequency.value = 600;
gain.gain.value = 0.3;
osc.connect(gain);
gain.connect(ctx.destination);
osc.start();
ctx.resume();
</script>
<body>tone 600 Hz</body>
"""


def scenario_chrome(ctx: Context) -> dict:
    if not ctx.audible:
        # Chrome picks the default output device and cannot be routed from here.
        raise NeedsConsent("Chrome plays on the default output device; use --audible")
    if not CHROME.exists():
        return {"pass": False, "error": f"chrome not found at {CHROME}"}
    cdir = ctx.work / "chrome"
    if cdir.exists():
        shutil.rmtree(cdir, ignore_errors=True)
    cdir.mkdir(parents=True)
    page = cdir / "tone.html"
    page.write_text(CHROME_PAGE, "utf-8")
    profile = cdir / "profile"
    marker = str(profile)
    outside = ctx.player(TONES["outside"])
    proc = subprocess.Popen(
        [
            str(CHROME),
            f"--user-data-dir={profile}",
            "--autoplay-policy=no-user-gesture-required",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--window-size=320,200",
            page.as_uri(),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    root = None
    try:
        deadline = time.time() + 30
        sess_pids: list[int] = []
        while time.time() < deadline and not sess_pids:
            time.sleep(1.0)
            table = process_table()
            roots = [
                p
                for p in table
                if p["Name"] == "chrome.exe"
                and marker in (p["CommandLine"] or "")
                and "--type=" not in p["CommandLine"]
            ]
            if not roots:
                continue
            root = roots[0]["ProcessId"]
            mine = descendants(root, table)
            sess_pids = [
                s["pid"] for s in ctx.rec.sessions() if s["pid"] in mine and s["state"] == "Active"
            ]
        if not root or not sess_pids:
            return {"pass": False, "error": "no active chrome session found", "root": root}
        table = process_table()
        mine = descendants(root, table)
        types = {}
        for p in table:
            if p["ProcessId"] in mine:
                cl = p["CommandLine"] or ""
                kind = next(
                    (a.split("=", 1)[1] for a in cl.split() if a.startswith("--type=")), "browser"
                )
                sub = next(
                    (a.split("=", 1)[1] for a in cl.split() if a.startswith("--utility-sub-type=")),
                    "",
                )
                types[p["ProcessId"]] = f"{kind}{':' + sub if sub else ''}"
        if not ctx.rec.wait_session(outside.pid):
            return {"pass": False, "error": "outside player has no session"}
        time.sleep(1.0)
        session_pid = sess_pids[0]
        res = ctx.rec.record(
            ctx.work / "chrome-rec",
            10,
            ["--pid", str(root), "--pid", str(session_pid), "--system"],
        )
    finally:
        # Only the instance started here is stopped: its command line holds the profile path.
        if root:
            run(["taskkill", "/PID", str(root), "/T", "/F"])
        with contextlib.suppress(Exception):
            proc.kill()
        outside.stop()
        shutil.rmtree(profile, ignore_errors=True)
    freqs = {"chrome_tone": 600, "outside_tone": TONES["outside"]}
    root_rep = tone_report(read_track(ctx.work / "chrome-rec", f"pid-{root}")[1], 600, freqs)
    sess_rep = tone_report(read_track(ctx.work / "chrome-rec", f"pid-{session_pid}")[1], 600, freqs)
    for rep in (root_rep, sess_rep):
        rep["pass"] = bool(rep["own_present"] and rep["worst_other_db"] <= ISOLATION_LIMIT_DB)
    return {
        "browser_root_pid": root,
        "audio_session_owner_pids": {str(p): types.get(p, "?") for p in sess_pids},
        "process_roles": {str(p): t for p, t in sorted(types.items())},
        "record_browser_root_with_tree": root_rep,
        "record_audio_session_process_only": sess_rep,
        "recommendation": "record the browser root process with its tree: the audio is played by "
        "a child utility process (audio service), which the tree includes",
        "run": summarize_run(res),
        "pass": bool(root_rep["pass"]),
    }


def scenario_align(ctx: Context) -> dict:
    dur = 20 if ctx.quick else 24
    a = ctx.player(click=True, limit_s=60, volume=50)
    time.sleep(0.37)
    b = ctx.player(click=True, limit_s=60, volume=50)
    try:
        if not (ctx.rec.wait_session(a.pid) and ctx.rec.wait_session(b.pid)):
            return {"pass": False, "error": "click players have no session"}
        res = ctx.rec.record(
            ctx.work / "align",
            dur,
            ["--system", "--pid", str(a.pid), "--pid", str(b.pid)]
            + (["--mic-device", ctx.mic_device] if ctx.mic_device else []),
        )
    finally:
        a.stop()
        b.stop()
    d = ctx.work / "align"
    oa, xa = read_track(d, f"pid-{a.pid}")
    ob, xb = read_track(d, f"pid-{b.pid}")
    os_, xs = read_track(d, "system")
    ta, tb, ts = click_times_ms(oa, xa), click_times_ms(ob, xb), click_times_ms(os_, xs)
    da = pair_clicks(ta, ts, tb)
    db = pair_clicks(tb, ts, ta)
    out = {
        "run": summarize_run(res),
        "clicks_found": {"a": len(ta), "b": len(tb), "system": len(ts)},
    }
    if len(da) < 5 or len(db) < 5:
        out.update(
            {
                "pass": False,
                "error": "too few matched clicks",
                "matched": {"a": len(da), "b": len(db)},
            }
        )
        return out
    mean_a, mean_b = float(np.mean(da)), float(np.mean(db))
    out.update(
        matched={"a": len(da), "b": len(db)},
        track_a_minus_system_ms={
            "mean": round(mean_a, 2),
            "std": round(float(np.std(da)), 2),
            "max_abs": round(float(np.max(np.abs(da))), 2),
        },
        track_b_minus_system_ms={
            "mean": round(mean_b, 2),
            "std": round(float(np.std(db)), 2),
            "max_abs": round(float(np.max(np.abs(db))), 2),
        },
    )
    spread = float(
        max(np.max(np.abs(np.array(da) - mean_b)), np.max(np.abs(np.array(db) - mean_a)))
    )
    out["track_a_vs_b_offset_ms"] = round(abs(mean_a - mean_b), 2)
    out["worst_alignment_error_ms"] = round(
        max(abs(mean_a - mean_b), float(np.max(np.abs(da))), float(np.max(np.abs(db)))), 2
    )
    out["worst_pairwise_error_ms"] = round(spread, 2)
    if ctx.mic_device:
        om, xm = read_track(d, "mic")
        tm = click_times_ms(om, xm)
        dma, dmb = pair_clicks(ta, tm, tb), pair_clicks(tb, tm, ta)
        out["microphone_track"] = {
            "device": ctx.mic_device,
            "clicks_found": len(tm),
            "matched": {"a": len(dma), "b": len(dmb)},
            "track_a_minus_mic_ms": round(float(np.mean(dma)), 2) if dma else None,
            "track_b_minus_mic_ms": round(float(np.mean(dmb)), 2) if dmb else None,
            "note": "informational: the virtual cable adds its own buffering latency",
        }
    out["limit_ms"] = ALIGNMENT_LIMIT_MS
    out["method"] = (
        "Two ffplay processes play the same click file started at different times; "
        "every click is located in its own track and in the system loopback track "
        "(which holds both). Deltas are on the session clock (track offset + "
        "sample position)."
    )
    out["pass"] = bool(out["worst_alignment_error_ms"] <= ALIGNMENT_LIMIT_MS)
    return out


def scenario_crash(ctx: Context) -> dict:
    """Kill the recorder in the middle of a segment and check what survives. Silent source."""
    d = ctx.work / "crash"
    if d.exists():
        shutil.rmtree(d)
    t = SilentProcess(300)
    try:
        proc = subprocess.Popen(
            [
                str(ctx.rec.exe),
                "record",
                "--out",
                str(d),
                "--duration",
                "120",
                "--pid",
                str(t.pid),
                "--segment-s",
                "10",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(26.5)
        proc.kill()
        proc.wait()
    finally:
        t.stop()
    journal = (d / "journal.jsonl").read_text("utf-8")
    segs = [json.loads(line) for line in journal.splitlines() if '"event":"segment"' in line]
    done = sum(s["samples"] for s in segs) / RATE
    on_disk = 0.0
    for f in sorted((d / f"pid-{t.pid}").glob("seg-*.wav")):
        on_disk += max(0, f.stat().st_size - 44) / 2 / RATE
    replay = run([str(ctx.rec.exe), "replay", str(d)])
    lost = 26.5 - on_disk
    return {
        "recorded_before_kill_s": 26.5,
        "journaled_complete_segments_s": round(done, 2),
        "audio_readable_on_disk_s": round(on_disk, 2),
        "audio_lost_s": round(lost, 2),
        "limit_s": KILL_LOSS_LIMIT_S,
        "replay_output": replay.stdout.strip().splitlines()[-4:],
        "pass": bool(lost <= KILL_LOSS_LIMIT_S and done >= 20),
    }


def scenario_apps(ctx: Context) -> dict:
    table = process_table()
    sessions = ctx.rec.sessions()
    wanted = {
        "teams": ("ms-teams.exe", "teams.exe", "msteams.exe"),
        "discord": ("discord.exe",),
        "chrome": ("chrome.exe",),
    }
    out = {}
    for key, names in wanted.items():
        procs = [p for p in table if (p["Name"] or "").lower() in names]
        sess = [s for s in sessions if s["process"].lower() in names]
        out[key] = {
            "processes_running": len(procs),
            "in_sessions": len(sess),
            "session_states": sorted({s["state"] for s in sess}),
        }
    pkg = ps_json("Get-AppxPackage *Teams* | Select-Object Name,Version | ConvertTo-Json -Compress")
    out["teams"]["installed_packages"] = [f"{p['Name']} {p['Version']}" for p in pkg]
    discord_dir = Path(os.environ.get("LOCALAPPDATA", "")) / "Discord"
    out["discord"]["installed"] = (discord_dir / "Update.exe").exists()
    out["note"] = (
        "Only listing; nothing was started, no account was used. Apps that are not "
        "running cannot appear in the session list."
    )
    out["pass"] = None
    return out


def scenario_drift(ctx: Context) -> dict:
    """Clock drift between the microphone and loopback tracks. No sound is played: the
    loopback tracks follow a silent process, the microphone records the room. The microphone
    audio is deleted after the measurement unless --keep-mic-audio is given."""
    minutes = ctx.minutes if ctx.minutes else (2 if ctx.quick else 12)
    run_s = int(minutes * 60)
    silent = [SilentProcess(run_s + 120) for _ in range(2)]
    tone = None
    try:
        args = ["--mic-device", ctx.mic_device] if ctx.mic_device else ["--mic"]
        if ctx.no_drift_correction:
            args.append("--no-drift-correction")
        if ctx.mic_buffer_ms is not None:
            args += ["--mic-buffer-ms", str(ctx.mic_buffer_ms)]
        if ctx.mic_mmcss:
            args.append("--mic-mmcss")
        if ctx.mic_device:
            # A real signal for the microphone track: a tone on the output that the chosen
            # capture device carries.
            tone = ctx.player(TONES["a"], limit_s=run_s + 120, volume=20)
            if not ctx.rec.wait_session(tone.pid):
                return {"pass": False, "error": "tone player has no active session"}
            args += ["--pid", str(tone.pid)]
        for p in silent:
            args += ["--pid", str(p.pid)]
        res = ctx.rec.record(ctx.work / "drift", minutes * 60, args)
    finally:
        for p in silent:
            p.stop()
        if tone:
            tone.stop()
    s = res["summary"]
    tracks = {}
    for name, t in s.get("tracks", {}).items():
        tracks[name] = {
            "state": t["state"],
            "error": t["error"],
            "stored_s": round(t["stored_s"], 3),
            "clock_ratio_ppm": t["clock_ratio_ppm"],
            "timestamp_ratio_ppm": t["timestamp_ratio_ppm"],
            "dropped_buffers": t["dropped_buffers"],
            "continuity_ok": (t["continuity"] or {}).get("ok"),
            "silence_fill_events": t["stats"]["silence_fill_events"],
            "data_discontinuities": t["stats"]["data_discontinuities"],
            "drift_correction_ppm": t.get("drift_correction_ppm"),
            "max_packet_interval_ms": round(t["stats"]["max_packet_interval_ms"], 2),
        }
    pairs = s.get("drift_pairs", [])
    mic_pairs = [p for p in pairs if "mic" in (p["a"], p["b"]) and p["ms_over_run"] is not None]
    worst = max((abs(p["ms_over_run"]) for p in mic_pairs), default=None)
    mic_track = tracks.get("mic", {})
    mic_flags = mic_track.get("data_discontinuities")
    out = {
        "minutes": minutes,
        # The tuning the recorder reports it was given (all off by default), and the
        # data-discontinuity flags of the microphone track: the number the experiment is about.
        "mic_tuning": s.get("mic_tuning"),
        "mic_data_discontinuities": mic_flags,
        "mic_discontinuities_per_minute": (
            round(mic_flags / minutes, 3) if mic_flags is not None else None
        ),
        "run": summarize_run(res),
        "tracks": tracks,
        "pairs": pairs,
        "worst_pair_drift_ms_over_run": worst,
        "projected_ms_over_10h": max((abs(p["ms_over_10h"]) for p in mic_pairs), default=None),
        "projected_ms_over_2h": max(
            (abs(p["ppm_difference"]) * 7.2 for p in mic_pairs), default=None
        ),
        "note": "drift = rate difference of the sources measured on packet arrival times; "
        "the alignment target is 50 ms, so sources must stay within it over a recording",
        "pass": None,
    }
    over_2h = out["projected_ms_over_2h"]
    # The P1 recording lasts 2 hours, so the drift is judged over that length.
    out["pass"] = bool(
        over_2h is not None and over_2h <= ALIGNMENT_LIMIT_MS and not s["totals"]["failed_tracks"]
    )
    mic_dir = ctx.work / "drift" / "mic"
    if ctx.mic_device and mic_dir.exists():
        x = read_track(ctx.work / "drift", "mic")[1]
        out["mic_tone"] = tone_report(x, TONES["a"], TONES)
        out["mic_device"] = ctx.mic_device
    if mic_dir.exists() and not ctx.keep_mic_audio:
        shutil.rmtree(mic_dir)
        out["mic_audio_deleted"] = True
    return out


# ---------------------------------------------------------------- soak


def cim_start(cmdline: str) -> int:
    """Start a process outside this process tree so that it survives the caller."""
    script = (
        "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create "
        f"-Arguments @{{CommandLine='{cmdline}'}}; "
        "if ($r.ReturnValue -ne 0) { throw 'create failed' }; $r.ProcessId"
    )
    out = run(["powershell", "-NoProfile", "-Command", script])
    if out.returncode != 0:
        raise RuntimeError(out.stderr)
    return int(out.stdout.strip().splitlines()[-1])


def soak_start(ctx: Context, hours: float, mic: bool) -> dict:
    """Start a long recording in the background, outside this process tree.

    Three silent processes are recorded (plus the microphone with ``mic``): the run measures
    continuity, dropped buffers, CPU and memory without playing any sound.
    """
    d = ctx.data / "p1-soak"
    if d.exists():
        shutil.rmtree(d)
    d.mkdir(parents=True, exist_ok=True)
    secs = int(hours * 3600)
    pids = [cim_start(f"ping -n {secs + 600} 127.0.0.1") for _ in range(3)]
    args = f'--out "{d}" --duration {secs}' + "".join(f" --pid {p}" for p in pids)
    if mic:
        args += " --mic"
    rec_pid = cim_start(f'"{ctx.rec.exe}" record {args}')
    info = {
        "started_utc": datetime.now(UTC).isoformat(timespec="seconds"),
        "hours": hours,
        "silent_source_pids": pids,
        "recorder_pid": rec_pid,
        "out": "benchmarks/data/p1-soak",
        "mic": mic,
        "tones_hz": {},
    }
    (d / "soak-launch.json").write_text(json.dumps(info, indent=2), "utf-8")
    return info


def soak_report(ctx: Context) -> dict:
    d = ctx.data / "p1-soak"
    launch_path, summary_path = d / "soak-launch.json", d / "summary.json"
    if not launch_path.exists():
        return {"status": "not started", "pass": None}
    launch = json.loads(launch_path.read_text("utf-8"))
    if not summary_path.exists():
        return {"status": "running", "launch": launch, "pass": None}
    s = json.loads(summary_path.read_text("utf-8"))
    out = {
        "status": "complete" if s["complete"] else "running",
        "launch": launch,
        "elapsed_s": round(s["elapsed_s"], 1),
        "requested_s": s["requested_duration_s"],
        "dropped_buffers": s["totals"]["dropped_buffers"],
        "timestamp_errors": s["totals"]["timestamp_errors"],
        "silence_fill_events": s["totals"]["silence_fill_events"],
        "failed_tracks": s["totals"]["failed_tracks"],
        "cpu_avg_percent_total": round(s["cpu"]["avg_percent_total"], 4),
        "cpu_max_5s_window_percent_total": round(s["cpu"]["max_window_percent_total"], 4),
        "peak_working_set_mb": round(s["memory"]["peak_working_set_mb"], 2),
        "drift_pairs": s["drift_pairs"],
        "tracks": {},
    }
    ok = True
    for name, t in s["tracks"].items():
        cont = t["continuity"] or {}
        out["tracks"][name] = {
            "state": t["state"],
            "segments": cont.get("segments"),
            "continuity_ok": cont.get("ok"),
            "stored_s": round(t["stored_s"], 2),
            "dropped_buffers": t["dropped_buffers"],
            "clock_ratio_ppm": t["clock_ratio_ppm"],
        }
        ok &= bool(cont.get("ok"))
    if s["complete"]:
        # Silent sources: the loopback tracks must be silent, not just present.
        for name in list(out["tracks"]):
            if name.startswith("pid-"):
                x = read_track(d, name)[1]
                rms = float(np.sqrt(np.mean(x**2))) if len(x) else 0.0
                out["tracks"][name]["rms_dbfs"] = round(20 * math.log10(max(rms, 1e-9)), 1)
        pairs = [abs(p["ms_over_run"]) for p in s["drift_pairs"] if p["ms_over_run"] is not None]
        out["worst_drift_ms_over_run"] = max(pairs) if pairs else None
        out["pass"] = bool(
            ok
            and out["dropped_buffers"] == 0
            and not out["failed_tracks"]
            and out["cpu_avg_percent_total"] <= CPU_LIMIT_PERCENT
            and out["peak_working_set_mb"] <= MEMORY_LIMIT_MB
        )
    else:
        out["pass"] = None
    return out


# ---------------------------------------------------------------- results


def evaluate(results: dict) -> dict:
    sc = results.get("scenarios", {})
    soak = sc.get("soak", {})
    iso = sc.get("isolation", {})
    chrome = sc.get("chrome", {})
    ali = sc.get("align", {})
    drift = sc.get("drift", {})
    ver = sc.get("version", {})
    crash = sc.get("crash", {})

    def status(p) -> str:
        return {True: "pass", False: "fail", None: "not measured"}[p]

    run_drops = [
        r["run"].get("dropped_buffers")
        for r in sc.values()
        if isinstance(r, dict) and isinstance(r.get("run"), dict)
    ]
    short_cpu = [
        r["run"].get("cpu_avg_percent_total")
        for r in sc.values()
        if isinstance(r, dict) and isinstance(r.get("run"), dict)
    ]
    crit = {
        "no_dropped_buffers_2h": {
            "target": "0 dropped buffers over 2 h",
            "measured": {
                "soak": soak.get("dropped_buffers"),
                "short_runs_total": sum(d for d in run_drops if d is not None),
            },
            "status": status(
                None if soak.get("status") != "complete" else soak.get("dropped_buffers") == 0
            ),
            "note": "short runs are informational; the 2 h run decides"
            if soak.get("status") != "complete"
            else "",
        },
        "tracks_contain_only_own_audio": {
            "target": f"other tones at or below {ISOLATION_LIMIT_DB} dB relative to own tone",
            "measured": {
                "three_ffplay_processes": iso.get("tracks"),
                "include_tree_child_process": sc.get("tree", {}).get("include_tree_on_parent_cmd"),
                "chrome_browser_tree": chrome.get("record_browser_root_with_tree"),
            },
            "status": status(
                all(x.get("pass") for x in (iso, sc.get("tree", {}), chrome) if x)
                if (iso and chrome and sc.get("tree"))
                else None
            ),
        },
        "alignment_error_ms": {
            "target": f"{ALIGNMENT_LIMIT_MS} ms or less",
            "measured": {
                "click_test_worst_ms": ali.get("worst_alignment_error_ms"),
                "drift_over_run_ms": drift.get("worst_pair_drift_ms_over_run"),
                "projected_drift_over_2h_ms": drift.get("projected_ms_over_2h"),
                "projected_drift_over_2h_ms_uncorrected": sc.get("drift_uncorrected", {}).get(
                    "projected_ms_over_2h"
                ),
                "drift_run_minutes": drift.get("minutes"),
            },
            "status": status(
                None if not ali or not drift else bool(ali.get("pass") and drift.get("pass"))
            ),
        },
        "cpu_percent_total": {
            "target": f"{CPU_LIMIT_PERCENT} % or less",
            "measured": {
                "soak_avg": soak.get("cpu_avg_percent_total"),
                "short_runs": [c for c in short_cpu if c is not None],
            },
            "status": status(
                None
                if soak.get("status") != "complete"
                else soak.get("cpu_avg_percent_total") <= CPU_LIMIT_PERCENT
            ),
            "note": "short runs: "
            + (
                f"max {max(c for c in short_cpu if c is not None):.3f} %"
                if any(c is not None for c in short_cpu)
                else "none"
            ),
        },
        "memory_mb": {
            "target": f"{MEMORY_LIMIT_MB} MB or less",
            "measured": {"soak_peak": soak.get("peak_working_set_mb")},
            "status": status(
                None
                if soak.get("status") != "complete"
                else soak.get("peak_working_set_mb") <= MEMORY_LIMIT_MB
            ),
        },
        "audio_lost_on_kill_s": {
            "target": f"{KILL_LOSS_LIMIT_S} s or less",
            "measured": crash.get("audio_lost_s"),
            "status": status(crash.get("pass") if crash else None),
        },
        "works_on_minimum_windows_build": {
            "target": "application loopback works on the minimum supported build",
            "measured": {
                "build_tested": ver.get("windows_version"),
                "activation_ok": ver.get("activation_ok_all"),
            },
            "status": status(ver.get("pass") if ver else None),
            "note": "Windows 10 build 19045 is the oldest build available for testing; "
            "Microsoft documents build 20348, so 19045 working extends the supported range",
        },
    }
    return crit


def save_results(results_dir: Path, new: dict) -> Path:
    results_dir.mkdir(parents=True, exist_ok=True)
    build = platform.version().split(".")[-1]
    path = results_dir / f"p1-audio-win-{build}.json"
    data = json.loads(path.read_text("utf-8")) if path.exists() else {"scenarios": {}}
    data["scenarios"].update(new)
    data["prototype"] = "P1"
    data["generated_utc"] = datetime.now(UTC).isoformat(timespec="seconds")
    data["windows"] = {
        "version": platform.version(),
        "release": platform.release(),
        "machine": "reference machine class: RTX 3080, Windows 10 Pro",
    }
    data["criteria"] = evaluate(data)
    data["notes"] = [
        "isolation, tree, chrome and align were run once in quick mode (10 to 20 s) with ffplay "
        "(volume 40 %) and an isolated Chrome instance on the default output device; "
        "further audible runs wait for the user's consent",
        "drift and the long soak use silent source processes: they measure continuity, dropped "
        "buffers, CPU, memory and clock drift, not track content",
        "the microphone is recorded in drift only; the 2 h soak runs without it unless started "
        "with --mic",
    ]
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", "utf-8")
    return path


SCENARIOS = {
    "version": scenario_version,
    "isolation": scenario_isolation,
    "tree": scenario_tree,
    "chrome": scenario_chrome,
    "align": scenario_align,
    "crash": scenario_crash,
    "apps": scenario_apps,
}
EXPLICIT = ("drift", "soak-start", "soak-report")
AUDIBLE = ("isolation", "tree", "chrome", "align")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--exe", type=Path, required=True, help="path to the recorder binary")
    ap.add_argument("--data-dir", type=Path, default=DEFAULT_DATA)
    ap.add_argument("--results-dir", type=Path, default=DEFAULT_RESULTS)
    ap.add_argument("--quick", action="store_true", help="shorter recordings")
    ap.add_argument("--hours", type=float, default=2.0, help="length of soak-start")
    ap.add_argument("--mic", action="store_true", help="soak-start: also record the microphone")
    ap.add_argument("--keep-mic-audio", action="store_true")
    ap.add_argument(
        "--no-drift-correction",
        action="store_true",
        help="drift: record without correction (stored as drift_uncorrected)",
    )
    ap.add_argument(
        "--mic-buffer-ms",
        type=int,
        help="drift: open the microphone with an event buffer of this many ms (experiment)",
    )
    ap.add_argument(
        "--mic-mmcss",
        action="store_true",
        help="drift: register the microphone capture thread with MMCSS (experiment)",
    )
    ap.add_argument(
        "--minutes",
        type=float,
        help="drift: recording length in minutes (default 12, or 2 with --quick)",
    )
    ap.add_argument(
        "--mic-device",
        help="capture device used as the microphone track (e.g. a virtual cable output)",
    )
    ap.add_argument(
        "--audible",
        action="store_true",
        help="allow scenarios that play sound on the default output device",
    )
    ap.add_argument("--output-device", help="play test signals on this named output device")
    ap.add_argument(
        "scenarios",
        nargs="*",
        help=f"any of {', '.join([*SCENARIOS, *EXPLICIT])}; default: all but drift and soak",
    )
    args = ap.parse_args()
    args.exe = args.exe.resolve()

    if not shutil.which("ffmpeg") or not shutil.which("ffplay"):
        print("ffmpeg and ffplay must be on PATH", file=sys.stderr)
        return 2
    ctx = Context(Recorder(args.exe), args.data_dir, args.quick, args.audible, args.output_device)
    ctx.keep_mic_audio = args.keep_mic_audio
    ctx.mic_device = args.mic_device
    ctx.no_drift_correction = args.no_drift_correction
    ctx.mic_buffer_ms = args.mic_buffer_ms
    ctx.mic_mmcss = args.mic_mmcss
    ctx.minutes = args.minutes
    names = args.scenarios or [
        n for n in SCENARIOS if n not in AUDIBLE or args.audible or args.output_device
    ]
    new: dict = {}
    failed = False
    for name in names:
        print(f"== {name}", flush=True)
        if name == "soak-start":
            info = soak_start(ctx, args.hours, args.mic)
            print(json.dumps(info, indent=2))
            continue
        if name == "soak-report":
            result = soak_report(ctx)
            new["soak"] = result
        elif name == "drift":
            result = scenario_drift(ctx)
            new["drift_uncorrected" if ctx.no_drift_correction else "drift"] = result
        elif name in SCENARIOS:
            try:
                result = SCENARIOS[name](ctx)
            except NeedsConsent as e:
                print(f"skipped: {e}")
                continue
            new[name] = result
        else:
            print(f"unknown scenario {name}", file=sys.stderr)
            return 2
        print(json.dumps(result, indent=2, ensure_ascii=False))
        if result.get("pass") is False:
            failed = True
    if new:
        path = save_results(args.results_dir, new)
        print(f"results: {path}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
