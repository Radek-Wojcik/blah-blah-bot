# Third-party components of the TTS tool

This tool (`benchmarks/tools/tts`) is development tooling. It is not shipped with the application. Generated audio and model weights are never committed (`benchmarks/data/` is ignored). Versions below are the ones pinned in `uv.lock`.

## Models

| Component | Version / revision | License | Source |
|---|---|---|---|
| Chatterbox Multilingual weights (`t3_mtl23ls_v2`, `s3gen`, `ve`) | HF repo `ResembleAI/chatterbox`, revision `main` (sha `5bb1f6ee58e5` when checked on 2026-11-13); ungated | MIT (model card) | https://huggingface.co/ResembleAI/chatterbox |
| Piper voice `en_US-libritts-high` (904 speakers), used only to render reference clips | HF repo `rhasspy/piper-voices`, path `en/en_US/libritts/high/` | CC BY 4.0 (dataset LibriTTS, OpenSLR 60). Attribution required: LibriTTS, Zen et al. 2019 | https://huggingface.co/rhasspy/piper-voices/tree/main/en/en_US/libritts/high , https://www.openslr.org/60/ |

Notes:
- Chatterbox embeds an inaudible Perth watermark in every generated waveform. The tool keeps it enabled.
- The reference clips are synthetic speech produced by Piper from LibriTTS-trained voices; they inherit the CC BY 4.0 attribution requirement of the voice model.

## Direct dependencies

| Package | Version | License | Source |
|---|---|---|---|
| chatterbox-tts | 0.1.7 | MIT | https://pypi.org/project/chatterbox-tts/0.1.7/ |
| piper-tts | 1.8.0 | GPL-3.0-or-later (bundles espeak-ng, GPL-3.0) | https://pypi.org/project/piper-tts/1.8.0/ |
| torch | 2.6.0+cu124 | BSD-3-Clause | https://download.pytorch.org/whl/cu124 |
| torchaudio | 2.6.0+cu124 | BSD-2/3-Clause | https://download.pytorch.org/whl/cu124 |
| numpy | 1.26.4 | BSD-3-Clause | https://pypi.org/project/numpy/ |
| scipy | 1.17.1 | BSD-3-Clause | https://pypi.org/project/scipy/ |
| soundfile | 0.14.0 | BSD-3-Clause | https://pypi.org/project/soundfile/ |
| huggingface-hub | 1.33.0 | Apache-2.0 | https://pypi.org/project/huggingface-hub/ |
| setuptools | 80.10.2 (pinned `<81`) | MIT | https://pypi.org/project/setuptools/ |

## Notable transitive dependencies (via chatterbox-tts)

| Package | Version | License |
|---|---|---|
| transformers | 5.2.0 | Apache-2.0 |
| diffusers | 0.29.0 | Apache-2.0 |
| librosa | 0.11.0 | ISC |
| resemble-perth | 1.0.1 | MIT |
| s3tokenizer | 0.3.0 | Apache-2.0 |
| conformer | 0.3.2 | MIT |
| safetensors | 0.5.3 | Apache-2.0 |
| onnxruntime | 1.32.0 | MIT |
| pykakasi | 2.3.0 | GPL-3.0-or-later |
| gradio | 6.8.0 | Apache-2.0 |

## Copyleft note for NOTICE

`piper-tts` (with espeak-ng) and `pykakasi` are GPL-3.0-or-later. They are used only as local build-time tools in a separate environment: the reference clips come out of Piper, and no GPL code is linked into or distributed with the application. If audio produced here is ever redistributed, include the LibriTTS attribution above.

## Engines deliberately not used

- Coqui XTTS (non-commercial license), MMS-TTS (CC BY-NC) and edge-tts (online service).
