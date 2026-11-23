# 0022. Meeting detection: rules, sensitivity and the default

**Status:** Proposed (Stage 6)

## Context

After setup the user should only press Record and Stop. The application can notice a call and ask whether to record it, but a wrong prompt is a nuisance and a missed one defeats the purpose. The signals available without capturing any audio are the audio sessions of Windows (which program has a playback or a microphone stream open, whether it runs, a coarse peak) and, for Google Meet, the report of the browser extension ([ADR 0020](0020-bridge-protocol-and-pairing.md)). Browsers cannot be told apart by their audio sessions: a voice search, a video and a video call look the same, so browsers are not detected that way.

## Decision

### Rules (`crates/recorder/src/detect.rs`, pure logic)

Per call the detector moves through `Idle -> Candidate -> Prompted -> Cooldown`.

- **Candidate:** a known conferencing application (Discord, Discord PTB, `ms-teams`, `teams`, Zoom, Slack; the list is a setting) has an active microphone (capture) session. The state is dropped as soon as the microphone is released.
- **Prompted:** the condition has held for the sustain time of the sensitivity and the playback condition of the sensitivity is met. One prompt is raised for the call; only one prompt is on screen at a time.
- **Cooldown:** the user answered, or the application is being recorded. No second prompt for the same call. The call is over when the microphone has been unused for the end-of-call time; a back-to-back call after a gap shorter than that is the same call. After "Not now" the call stays quiet to its end, and with low sensitivity for 30 minutes more.
- A prompt that nobody answers is withdrawn when the call ends. A prompt is withdrawn also when the application starts being recorded or the user opts out in the settings.
- Per-application opt-out ("Never for Discord") is a list in the settings; the list of known applications can be edited. Browsers (and `msedgewebview2` itself) are excluded from the audio-session rule even when the user adds one to the list.
- Google Meet: the extension reports `meet{joined}` for a tab on a call URL that is audible and `left` when it leaves. The same engine counts the time since `joined` (the "Meet audible" time below) and ends the call at `left`.
- The application of an audio session is found through the parent chain of its process: the topmost known application among the ancestors wins (the new Teams plays its audio from an `msedgewebview2` child of `ms-teams.exe`); an unknown program stands for itself. Only the owners of new sessions and their parents are looked up; the answer is kept until the session is gone.

### Sensitivity

All numbers live in `recorder::detect::Presets` and the choice in `settings.detection.sensitivity`, so a different default is a change of configuration, not of rules.

| | Low | **Balanced (default)** | High |
|---|---|---|---|
| Microphone in use before the prompt | 20 s | 8 s | 3 s |
| Playback condition of the application | session active and sound lately | session exists | none |
| Meet tab audible before the prompt | 30 s | 10 s | 3 s |
| Call over after the microphone was unused | 120 s | 60 s | 30 s |
| After "Not now" | rest of the call and 30 min | rest of the call | rest of the call |
| Poll interval | 2 s | 1 s | 1 s |

Targets: no prompt from idle applications; exactly one prompt per call; delay of at most the sustain time plus 2 s; CPU of the detector at most 0.1 %.

**Default: balanced.** It prompts within about 10 s of a call starting while not asking for a microphone use of less than 8 s. Choosing a default is a stop-and-ask item of the roadmap, so this record stays Proposed until the user confirms it.

### Measurement

`recorder/examples/record_session_trace.rs` records session traces (enumeration only: sessions, state, coarse peak; no audio, no device or display names) and anonymizes them (process ids by order, programs other than the known conferencing ones as `app-N`). `recorder/examples/score_detection.rs` replays the traces in `crates/recorder/tests/traces/detect` through the detector once per sensitivity and writes `benchmarks/results/stage6-detection-<preset>.json`.

What the numbers say, and what they do not:

| | Low | Balanced | High |
|---|---|---|---|
| False prompts, real idle trace (4 min, one machine) | 0 | 0 | 0 |
| Labelled calls prompted, synthetic traces (of 6) | 5 | 6 | 6 |
| Delay from call start to prompt, synthetic | 20 s, Meet 30 s | 8 s, Meet 10 s | 3 s |
| False prompts, synthetic non-call traces | 0 | 1 (a 12 s Discord microphone test) | 2 (that test, a 6 s Teams device check) |
| Duplicate prompts | 0 | 0 | 0 |
| Calls missed | 1 (the second of two calls 90 s apart, taken as one call) | 0 | 0 |

- The **real** trace is four minutes of one idle Windows 10 machine. It has no conferencing application at all, so it shows only that other programs' sessions raise nothing.
- The **synthetic** traces are made by hand. The session states in them (for example that Discord keeps its microphone stream open while muted, and the length of a device check) are assumptions about the applications. They test the rules, not the applications.
- **Not measured yet (to be measured):** an 8-hour working day of real use (record it with `record_session_trace --minutes 480`, add the anonymized file to the trace folder and score it again); real Discord, new and classic Teams and Zoom calls, mutes and device checks; Windows 11; how often Teams and Discord open the microphone outside calls. Until then the false-prompt rate of a working day is unknown, and so is whether a default other than balanced is better. The numbers of the table are the starting values of the sensitivities, not tuned results.
- **CPU:** a poll of every session of all devices took 2.6 ms on average over 4 minutes on the reference machine (22 ms for the first one, which looks the processes up); the process used about 0.11 % of one core at a 1 s poll (the counter has the 15 ms resolution of the scheduler tick, so this is an estimate). In total CPU that is below the 0.1 % target, but this is a measurement of one machine, not a proof; if it turns out too high the poll can follow audio session notifications instead of a timer.

### In the application

A polling thread (`crates/app/src/detection.rs`) feeds the detector once per poll interval and the Meet reports of the extension through the same detector. A prompt opens a small always-on-top window that does not take the focus; the buttons are Record, Not now and Never for the application. Record starts a recording of the microphone and the application with the report choice of the last recording; without a usable choice (no earlier recording, the cycle gone) the recorder window opens at the report mode gate. A recording that runs already gets the missing sources instead.

## Consequences

- No audio is opened to detect a call; the detector reads session lists only.
- A call in an application that keeps its microphone closed while muted is not detected. A short microphone test in a watched application (longer than the sustain time) raises a prompt; "Not now" or "Never" answers it.
- A call in a browser is found only through the Meet report; Teams in a browser, Zoom in a browser and similar are not detected.
- Changing the default or one number needs a measurement on real calls; the scorer makes that a replay of recorded traces.

## Alternatives considered

- **Prompt on any microphone use:** simple, but dictation, voice search and device checks would prompt all day.
- **Detect browsers through their audio sessions:** cannot tell a call from a voice search; Meet is reported by the extension instead.
- **Notifications of the audio session manager instead of a poll:** lower CPU, more code and harder to replay from a trace; kept as the answer if the poll turns out too costly.
- **Record without asking:** a recording of a call nobody agreed to record is not a default this tool should have.
