# P1 audio capture prototype

Prototype P1 of [docs/RISKS-AND-POC.md](../../docs/RISKS-AND-POC.md): can the microphone and individual applications be recorded as separate, segmented tracks on one session clock, with no lost buffers and almost no CPU? Design background: [ADR 0002](../../docs/decisions/0002-audio-capture.md).

The recorder is a Rust binary (`wasapi` crate, MIT). Every track is 16 kHz mono 16-bit PCM, stored as `seg-NNNNN.wav` segments (60 s by default) next to an append-only `journal.jsonl` and a `summary.json`.

## Build and run

Needs Windows 10 (build 19041 or newer), the stable Rust toolchain, and for the hardware tests `ffmpeg`, `ffplay` and `uv` on `PATH`.

```powershell
cd benchmarks/p1-audio
cargo test                       # logic tests, no audio hardware needed
cargo clippy --all-targets -- -D warnings
cargo run --release -- selftest  # silent hardware tests, see below
```

`selftest` starts `selftest/selftest.py` through `uv` (numpy only, installed on demand). Results accumulate in `benchmarks/results/p1-audio-win-<build>.json`; recordings go to `benchmarks/data/` (not tracked by git).

## Commands

| Command | Purpose |
|---|---|
| `record --out DIR --duration S [--mic \| --mic-device NAME] [--system] [--pid PID[:tree]]... [--segment-s 60] [--no-drift-correction] [--mic-buffer-ms MS] [--mic-mmcss]` | Record tracks (`--mic-device` picks a capture device by name, e.g. a virtual cable output). Stops after the duration, or when a file named `STOP` appears in `DIR`. `--mic-buffer-ms` and `--mic-mmcss` are an experiment on the microphone track only (event buffer of that many ms, and MMCSS `Pro Audio` registration of its capture thread); both fall back to the defaults with a warning if refused, and `summary.json` records the tuning (`mic_tuning`). |
| `sessions [--json]` | List audio sessions: process id, parent, process name, state, session volume, peak level. |
| `replay DIR` | Check a recording against its journal (segment numbering, offsets, file sizes). |
| `probe [--pid PID]` | Report whether application loopback can be activated on this Windows build, per format. |
| `devices` | List playback devices (no sound). |
| `play --device NAME (--tone HZ \| --click) --seconds S` | Test signal player. It never uses the default device: a device name is required. |
| `session-volume --pid PID [--level 0..1] [--mute]` | Set the session volume of a test player (Windows remembers it per application). |
| `selftest [scenario ...]` | Run the hardware tests. |

`--pid N` captures process N **and its child processes**. The Windows API has only two modes, "target process tree" and "everything except the target process tree"; there is no "target process alone". `--pid N:exclude` selects the second mode (track `pid-N-excl`). `--format` accepts `pcm` only; see FLAC below.

### Track layout

```
<out>/journal.jsonl            append-only, flushed after every entry
<out>/summary.json             rewritten every minute (complete=false) and at the end
<out>/<track>/seg-00000.wav    track = mic | system | pid-<pid> | pid-<pid>-excl
```

Journal entries (one JSON object per line, `event` field): `session_start`, `track_start` (source, pid, `offset_ms` on the session clock), `segment` (file, `offset_ms`, samples; written when a segment is complete), `discontinuity` (`data_discontinuity`, `timestamp_error`, `silence_filled`), `track_end`, `session_end`. The session clock is the Windows performance counter, so tracks that start at different moments share one time base: sample *i* of a track is at `track offset_ms + i / 16`. A crash loses at most the open segment; `replay` ignores a torn last line, and a segment left open by a crash is still a readable WAV (the length field is patched when a segment is closed).

`summary.json` holds, per track and in total: dropped buffers (data-discontinuity flags, the first packet of a stream excluded because the engine flags it by design), timestamp errors, silence filled, clock drift in ppm and between track pairs, journal continuity check; CPU of the recorder as percent of total CPU (average and worst 5 s window; the larger of a `GetProcessTimes` estimate and one from the exact process cycle counter at a calibrated clock rate, because the scheduler-tick based figure reads 0 for a thread that wakes for microseconds every 10 ms); peak working set.

## Measured on this prototype

Reference run: Windows 10 Pro build 19045, RTX 3080 machine. Full numbers and pass/fail per P1 criterion are in `benchmarks/results/p1-audio-win-19045.json`.

- `AudioClient::new_application_loopback_client` **works on Windows 10 build 19045**, for both modes, with a 16 kHz mono 16-bit format requested directly (`autoconvert` on, the audio engine resamples). Microsoft documents build 20348; the minimum supported build for the application is therefore not constrained by this API above 19045 (older builds could not be tested).
- Tracks of separate processes contain only their own tone: other test tones were at least 60 dB below the own tone (limit 40 dB), also for a child process behind `cmd /c`, with the outside process absent. The `:exclude` mode records everything else and leaves the target out.
- Chrome plays through a child *audio service* utility process. Recording the **browser root process** (the one without `--type=`) captures it; the tree is needed because Chrome has many processes and the one owning the audio session is not the root.
- Process loopback delivers a continuous stream of 10 ms packets even when the process is silent or suspended; packet timestamps are synthesized from the sample count, so a drop cannot be seen from them. Dropped buffers are counted from the data-discontinuity flag. Drift is measured from packet arrival times instead.
- The loopback signal is captured **after** the session volume: a muted or volume-0 player records as silence (RMS 74 at 100 %, 0.5 at 0 %). Muting a test player is therefore not a way to make a test silent. Windows also remembers the level per application, so a player that was once muted starts muted; `sessions` shows this in the VOLUME column and `session-volume` repairs it.
- Clock drift: a USB microphone ran 54 ppm fast against the performance counter and the loopback tracks (39 ms after 12 minutes, 390 ms projected over 2 hours, 1.95 s over 10 hours), which misses the 50 ms alignment target on long recordings. The recorder therefore corrects every source that has real device timestamps (microphone, system loopback): it measures the sample-clock ratio against the performance counter, trims it by the smoothed position error, and resamples with linear interpolation (`src/drift.rs`). In the same 12 minute test with correction the microphone ends within -0.3 ppm of the loopback tracks (0.2 ms over the run, 1.9 ms projected over 2 hours, 9.7 ms over 10 hours). A first version without the position trim left 15 ppm; the result files keep all three runs. `--no-drift-correction` stores the tracks as captured. Process loopback timestamps are synthetic and its stream followed the performance counter to within 0.03 ppm, so it is not corrected. The microphone logged a data-discontinuity flag about every 186 s (4 to 6 in 12 minutes, regular spacing), probably the audio engine adjusting that USB clock; it is counted as dropped, so the strict 0-dropped criterion is not met for that microphone in the 12 minute runs.
- Virtual cable: with the default output set to `CABLE Input`, process loopback per PID records the player's signal (RMS 162 for ffplay at volume 40 %, the session listed on that device) and `CABLE Output` carries it as a microphone. As a capture device it reports a data-discontinuity flag every ~22 ms while audio plays, so it is useful as a signal source for alignment but not as a dropped-buffer reference.
- 2 hour recording of three silent processes: 0 dropped buffers, 121 continuous segments per track, recorder CPU 0.027 % of total CPU (about 0.9 % of one core of 32), peak working set 10.1 MB.
- FLAC: not implemented in the recorder. Encoding with ffmpeg offline gives 47 % of the PCM size for a few seconds of ambient microphone audio, 77 % for white noise at -26 dBFS (worst case), 0.7 % for silence; encoding 10 minutes takes about 0.2 s. At that cost FLAC segments are worth adopting in the product through a pure-Rust encoder (license to be verified before use).

## Hardware tests (`selftest`)

| Scenario | Plays sound? | What it checks |
|---|---|---|
| `version` | no | Application loopback activates (Windows build recorded). |
| `crash` | no | Recorder killed in the middle of a segment: audio left readable, journal replay. |
| `apps` | no | Whether Teams, Discord and Chrome are installed, running, and listed in `sessions`. |
| `drift` | no | 12 minutes of the default microphone (or `--mic-device NAME`, with a tone on the output it carries) plus loopback tracks of silent processes: drift between sources, projected over 2 and 10 hours (`--quick`: 2 minutes). The microphone audio is deleted afterwards unless `--keep-mic-audio`. `--minutes N` overrides the length, `--mic-buffer-ms MS` / `--mic-mmcss` pass the microphone tuning through, and the result holds `mic_tuning`, `mic_data_discontinuities` and `mic_discontinuities_per_minute`. |
| `soak-start` / `soak-report` | no | Starts a 2 hour recording of three silent processes in the background (`--hours`, `--mic` to add the microphone); `soak-report` reads `summary.json`, checks continuity and the criteria. |
| `isolation` | **yes** | Three players with 440, 1000 and 2000 Hz plus a fourth, unrecorded 3000 Hz player; each track must hold only its own tone (other tones 40 dB or more below), the system loopback track must hold all four. |
| `tree` | **yes** | `cmd /c <player>` recorded through the parent in include mode; exclude mode; direct child pid. |
| `chrome` | **yes** | An isolated Chrome instance (own `--user-data-dir`, autoplay allowed) plays a 600 Hz oscillator; checks which process to record. Only that instance is stopped afterwards. |
| `align` | **yes** | Two players play the same click file, started at different times; every click is located in its own track and in the system loopback track, which holds both; the deltas on the session clock give the alignment error. |

Run the audible scenarios with the default output device set to a virtual cable that nobody monitors (for example VB-Audio `CABLE Input`) and pass `--mic-device "CABLE Output"` to get a real signal on the microphone track; `sessions` shows on which device a player's session lives. Scenarios that play sound refuse to run unless you choose where the sound goes: `--output-device NAME` renders the signals with the built-in `play` helper to the named device (for example a virtual cable that nobody monitors); `--audible` plays through the default output device with `ffplay` and, for `chrome`, Chrome. Without either, `selftest` runs only the silent scenarios. Test signals are quiet (ffplay volume 40 %), but they are audible on the default device.

The silent long runs measure continuity, dropped buffers, CPU, memory and clock drift. They do not exercise the audio content of the tracks; the content checks are the audible scenarios.

## Manual test with real applications

Run this during a real call or while playing audio in the application. Nothing here logs in, joins or changes anything; the recorder only reads audio. Use `selftest apps` first to see what is installed and running.

1. Start the application and begin the call (or play a video in Chrome). Then list the sessions:

   ```powershell
   cargo run --release -- sessions
   ```

   Find the application in the PROCESS column with state `Active`:
   - **Discord**: `Discord.exe`. Several `Discord.exe` processes exist; the one with the session is a child of the main process. Use the parent that is not itself a child of another `Discord.exe` (the PARENT column helps) and record it with its tree.
   - **Teams (new client)**: `ms-teams.exe` (WebView2 based); the same rule applies: record the root process.
   - **Chrome**: `chrome.exe`; use the root browser process (the one whose parent is not a `chrome.exe`). The session itself is owned by a child, which the tree includes.

2. Record 2 minutes of the call together with the microphone, naming the root process id from step 1:

   ```powershell
   cargo run --release -- record --out ../data/p1-manual --duration 120 --mic --pid <PID>
   ```

   Speak, let the other side speak, and play a notification sound from another application (it must not appear in the application track).

3. Check the result:
   - `cargo run --release -- replay ../data/p1-manual` prints `ok` and no problems.
   - `summary.json`: `totals.dropped_buffers` is 0 and no track has `state: failed`. If activation fails, the `error` field holds the Windows message; please report it together with the Windows build.
   - Listen to `../data/p1-manual/pid-<PID>/seg-00000.wav` and `../data/p1-manual/mic/seg-00000.wav`: the first must hold the remote voices and the application sounds, not your own voice (unless the application plays a sidetone) and not the other application's notification; the second must hold your voice.
   - Repeat with a second process id to check that two applications give two separate tracks.

4. Record the outcome per application (works / does not work, process that owned the session, any error text), including the Windows version.

## Known limits

- Per-tab capture in Chrome is not possible with this API; the whole browser tree is recorded.
- Drift correction needs real device timestamps, so it covers the microphone and system loopback; process loopback tracks are on the session clock by construction. The residual rate between tracks is reported in `summary.json` (`drift_pairs`) with projections over the run and 10 hours.
- Windows builds older than 19041 and Windows 11 were not tested here.
