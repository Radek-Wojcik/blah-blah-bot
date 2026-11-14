# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-14

### Added

- Architecture overview, roadmap to 1.0, testing strategy, risk register and architecture decision records.
- Measurement harness in `benchmarks/`: metrics for word error rate, diarization error rate and task extraction, a generator of scripted test meetings with planted tasks, and a unit-tested scoring code.
- Prototype of per-application audio capture on Windows (WASAPI process loopback) with clock drift correction.
- Prototypes and measurements for speech recognition, diarization, speaker recognition, structured extraction with local language models, slide description, Chrome tab capture and Python environment creation, with raw results under `benchmarks/results/`.
- Pipeline timing for a ten-hour recording.
- Continuous integration (lint, tests, secret scan, commit message check) and contribution templates.
- Measured results in the architecture decision records, license notices for models and tools.
- Stage 0 decision gate: Whisper large-v3-turbo as the primary speech recognition model and Gemma 4 12B as the default language model; ADRs 0002 to 0006 accepted. The Chrome tab capture test was repeated on an idle machine and met the 100 ms gap criterion (longest gap 47 ms), so ADR 0008 is accepted too.

