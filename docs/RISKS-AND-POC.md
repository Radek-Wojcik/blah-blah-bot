# Risks and prototypes

## 1. Risk register

Probability and impact: L low, M medium, H high.

| ID | Risk | P | I | Mitigation | Prototype |
|---|---|---|---|---|---|
| R1 | Diarization fails with many speakers in one mixed track | H | H | Separate source tracks; microphone track as the user; speaker count bounded by participants; voice profiles; fast merge in the labeling screen. No automatic separation is promised above about eight speakers in one track. | P3, P4 |
| R2 | Mixed Polish and English speech is translated or mistranscribed | M | H | Language setting per cycle; glossary prompt; alternative ASR model; translation rate tracked as a metric | P2 |
| R3 | Extraction quality in Polish is too low at a model size that fits 12 GB | M | H | Schema-constrained output; quote verification; uncertainty markers; mandatory draft review; model comparison set; cloud model as a per-meeting choice | P5 |
| R4 | Voice recognition assigns the wrong person | M | H | Conservative threshold; suggestions instead of automatic assignment for weak matches; unknown voices stay unknown; every assignment is editable | P4 |
| R5 | Chrome restricts tab capture to tabs where the extension was invoked | H (confirmed) | M | Guided per-tab activation for groups, built in Stage 6 (popup list with a status per tab, Next tab, one action per tab); whole-browser capture as fallback in the source picker. Open: a real tab for 30 minutes | P7 |
| R6 | Per-process capture misses audio from child or helper processes | M | M | Capture with the process tree; per-application test matrix; whole-system capture as a last resort | P1 |
| R7 | Processing a 10-hour meeting takes too long | M | M | Resumable stages; batching; faster ASR alternative; progress and time estimate in the queue | P9 |
| R8 | Python environment with GPU libraries is large and fragile | M | M | Environment created on first use with pinned versions; preflight check of driver and CUDA; clear error messages | P8 |
| R9 | VRAM is not released between stages | L | H | One process per model; supervisor kills on timeout; VRAM check before each stage | P9 |
| R10 | Clock drift between audio devices misaligns tracks | M | M | Shared session clock; offsets stored per segment; drift measured in the soak test | P1 |
| R11 | Gated or relicensed model becomes unavailable | L | H | Models are replaceable behind step interfaces; licenses reviewed at every release; alternatives recorded in the ADRs | — |
| R12 | Personal data is published by mistake | L | H | Warning before the first publication to a public repository; excluded passages; per-target approval rules | — |
| R13 | Tokens leak through logs or files | L | H | Credential Manager only; log redaction; secret scanning in CI | — |
| R14 | Meeting detection produces false prompts | M | L | Built in Stage 6: known applications only, microphone in use for a sustain time, one prompt per call, browsers left to the Meet report, opt-out per application, three sensitivities. Measured on traces only (0 false prompts on a 4 minute real idle trace; 1 on a synthetic non-call trace with balanced); a working day of real use is still to be measured | — |
| R15 | Unsigned installer triggers Windows warnings | H | L | Documented in the README; checksums published; code signing evaluated for 1.0 | — |
| R16 | External API changes (GitHub issue fields, Discord, Notion) | M | M | Thin adapters; mock servers in tests; release checks against live test targets | — |
| R17 | Scope: all requirements in 1.0 for one developer | H | M | Staged releases, each usable on its own; decision gates; optional integrations last | — |
| R18 | The extension is not in the Chrome Web Store: developer mode warning, Chrome may switch it off, no automatic update, scripted loading with `--load-extension` ignored by Chrome 137 or newer | H | L | Unpacked install from the release zip with a guide and troubleshooting table (ADR 0021); a fixed extension id and a protocol version check so an old extension is refused with a message | — |

## 2. Prototypes and success criteria

All prototypes run on the reference machine (RTX 3080 12 GB, 32 GB RAM, Windows 11) and write machine-readable results to `benchmarks/results/`. Metric definitions are in [TESTING.md](TESTING.md).

| ID | Prototype | Success criteria | If it fails |
|---|---|---|---|
| P1 | Record microphone plus Discord, Teams and Chrome as separate segmented tracks for 2 hours | No dropped buffers; tracks contain only their application's audio; alignment error 50 ms or less; CPU 3% or less; works on the minimum supported Windows build | Per-application workaround; raise minimum Windows version; whole-system capture for that application |
| P2 | Transcribe the scripted sets with faster-whisper large-v3 and Parakeet-TDT-0.6B-v3; language `auto` and forced; with and without glossary | WER 15% or less (Polish), 20% or less (mixed); term recall 85% or more; translation rate 1% or less; 10 h of audio in 60 min or less | Choose the other model; force the language per cycle; adjust the glossary strategy |
| P3 | Diarize scripted recordings with 2–4, 6–10 and 15+ voices, with and without speaker bounds | DER 15% or less (2–4), 30% or less (6–10); 10 h of audio in 60 min or less; 15+ reported only | Rely on source tracks and voice profiles; document the limit |
| P4 | Enroll voices from one recording and recognize them in another; include voices never enrolled | Speaker accuracy 90% or more for turns of 5 s or longer; false accept rate 2% or less | Raise the threshold and fall back to suggestions; try another embedding model |
| P5 | Run two or three candidate LLMs on the extraction set through `llama-server` | Schema validity 100%; task recall and precision 80% or more; assignee and due date 85% or more; quote validity 95% or more; 10-hour transcript in 90 min or less | Different model or quantization; smaller windows; prompt changes; document cloud option as recommended for long meetings |
| P6 | Describe the `slides` set with Qwen3-VL-8B | 90% or more of labeled key facts present; no invented numbers on charts in manual review; 5 s or less per image | Add an OCR step before the model; larger image token budget |
| P7 | Capture one Chrome tab for 30 minutes to a local WebSocket; then two tabs with one activation each | No gap longer than 100 ms; tab stays audible; clean stop and restart; second tab added without restarting the first | Whole-browser capture as the primary browser path |
| P8 | Create the Python environment with `uv` on a clean Windows machine | Unattended; reproducible from a lock file; size and time recorded; GPU detected | Ship a prebuilt environment archive as a release asset |
| P9 | Estimate the full pipeline for 10 hours from P2, P3, P5 and P6, then confirm on one generated 10-hour recording | 4 h or less; peak VRAM 11.5 GB or less; VRAM returns to baseline between stages | Faster ASR; fewer summary levels; process long meetings in the background over several sessions |

## 3. Decision gate

The gate closes Stage 0. Input is the measurement report with every row of the table above. Possible outcomes per component:

1. **Accept** the primary choice with measured values written into the architecture document.
2. **Switch** to the documented alternative.
3. **Adjust** a requirement or a threshold, with the reason recorded in the corresponding ADR.

No product code is written before the gate is closed.
