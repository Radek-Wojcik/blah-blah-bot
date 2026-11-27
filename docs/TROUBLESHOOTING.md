# Troubleshooting

Before a meeting is processed, the application checks the setup item by item. A missing piece is refused with a stable code and a message that says how to fix it. This page lists those codes, then other common problems. The first-run wizard ([USER-GUIDE.md](USER-GUIDE.md#first-run)) fixes each of them: the banner above the meeting list, the **Fix** buttons on the wizard summary and a refused processing all open it at the step that belongs to the code. Settings referred to here are in **Settings > Processing**, where **Run setup again** reopens the wizard.

## First-run checks

| Code | Typical message | What to do |
|---|---|---|
| `setup_worker_env` | "The processing environment has not been created yet." / "uv, the installer of the processing environment, is not available" / "The processing worker was not found" | Prepare the environment in the setup (step "Processing environment"). The first run needs a network connection and downloads several GB (about 7.4 GB on the reference machine). If `uv` is missing, install it or set the path to `uv.exe`. If the worker folder was not found, reinstall the application or set the worker folder. |
| `setup_model_missing` | "The speech recognition model (Whisper turbo) is not downloaded." | Download it in the setup (step "Speech models", about 1.6 GB). Models cannot be checked until the environment exists, so prepare that first. |
| `setup_hf_token` | "No Hugging Face token is stored. The speaker model needs one." | Accept the terms of the speaker model on Hugging Face, create a read token and enter it. After the model is downloaded the token is no longer needed. |
| `setup_llama_server` | "llama-server was not found on this computer." | See [llama-server not found](#llama-server-not-found). |
| `setup_llm_model` | "No language model was chosen, so no report can be written." | See [No language model chosen](#no-language-model-chosen). |
| `setup_vision` | "The screenshot model is not downloaded." | See [Screenshot model missing](#screenshot-model-missing). |

A job that fails later with the same cause shows the worker's own message, for example "The model 'vision' (...) is not downloaded. Download it in Settings > Processing or run the setup again." or "The model '...' is incomplete: missing ... Download it again in Settings > Processing or run the setup again." The fix is the same: download the model again.

### Screenshot model missing

`setup_vision`: "The screenshot model is not downloaded." Download it in the setup (about 5 GB), or turn **Describe screenshots** off. With it off the check reads "Screenshot analysis is turned off." and screenshots are kept but not described.

### llama-server not found

`setup_llama_server`: "llama-server was not found on this computer." The screenshot description and the local language model both use `llama-server`, which is part of llama.cpp and is not shipped with the application.

- Install llama.cpp, for example with `winget install ggml.LlamaCpp`, then detect it in the setup, or enter the full path to `llama-server.exe`.
- Detection looks at the path in the settings, the `BBB_LLAMA_SERVER` environment variable, the `PATH`, the WinGet folders and the usual install folders.
- If a path is set but unusable: "The llama-server path in the settings is not usable: ...". Choose a detected llama-server or enter the correct path.
- If the file was found but did not run: "llama-server at ... did not run: ...". Check the file or choose another one.
- If a job fails while starting it, the message reads "llama-server was not found at '...'" or "...on the PATH (looked for 'llama-server'). Set its location in Settings > Processing or run the setup again."
- If you only want a transcript, turn reports off and screenshot analysis off; the item then reads "llama-server is not needed with the current choices."

### No language model chosen

`setup_llm_model`: "No language model was chosen, so no report can be written." Choose one of:

- a `.gguf` model file for the local backend,
- an external OpenAI-compatible server (address and model name),
- or skip reports. Meetings then get a transcript only: "Reports are turned off: meetings get a transcript only." You can choose a model later and process again.

A chosen file that cannot be used reads "The chosen language model cannot be used: ..." followed by the reason. Choose the file again.

### Path errors

Paths for `uv`, `llama-server`, the language model and the worker folder are checked before they are saved. The code says what is wrong:

| Code | Meaning |
|---|---|
| `path_empty` | No path was given. |
| `path_not_absolute` | The path must start with a drive letter. Relative paths are refused. |
| `path_network` | Network and device paths are not accepted. |
| `path_not_found` | Nothing exists there, or a bare program name was not found on the PATH. Give the full path. |
| `path_not_file` / `path_not_dir` | A file was expected and a folder given, or the other way round. |
| `path_wrong_kind` | The file is not the expected kind: wrong file name, not a Windows program, not an `.exe`, not a `.gguf`, not a valid GGUF model, or a folder without the expected file. |

## Speaker model: 401 or 403 from Hugging Face

The speaker model is gated. The message "'...' is gated: accept its conditions on Hugging Face and add your access token in Settings > Processing or in the setup." means the token is missing. If a token is stored and the download is still refused, open the model's page on Hugging Face while signed in as the token's owner, accept the conditions, and download again. Use a token with read access.

## GPU waiting or out of memory

A job starts a stage only when the GPU is idle enough and enough video memory is free. Otherwise it waits and the queue shows the reason:

- "GPU is busy (N% utilization, limit 40%)": another program is using the GPU.
- "Not enough free GPU memory (X GB free, Y GB needed)": the stages need about 3.0 GB (transcription), 3.5 GB (speaker separation) and 9.5 GB (screenshots and the local language model).
- "Waiting for the GPU to stay idle (N of 10 s)": the GPU must stay below the limit for 10 seconds.
- "No NVIDIA GPU readings are available": the NVIDIA driver is missing or does not answer. Processing without an NVIDIA GPU is not supported, so the job waits.

Close other programs that use the GPU (games, video editors, browsers with hardware acceleration under load, other model servers). The measured peak is 8.5 GB for a 2 hour recording and 9.7 GB (absolute) during extraction of a 10 hour transcript, which leaves little room on a 12 GB card. With a smaller card, use a smaller model or an external server for reports.

## Installer

### SmartScreen warns about the installer

The installer is not signed ([ADR 0024](decisions/0024-installer-signing.md)), so Windows shows "Windows protected your PC". Compare the SHA256 checksum with `SHA256SUMS.txt` from the same release, then choose "More info" and "Run anyway". If your organization removes that choice, the installer cannot be run on that machine. See [INSTALL.md](INSTALL.md).

### WebView2 is missing

The installer does not download or bundle the Microsoft Edge WebView2 runtime (`webviewInstallMode: skip`), because Windows 11 and current Windows 10 already include it. If the application fails to open its window on an older Windows 10 machine, install the Evergreen WebView2 Runtime from Microsoft and launch the application again.

## Offline use

Processing needs no network once the environment and the models are in place. Cycle repositories work offline too; the application shows the state of the last pull. Creating the environment, downloading models, cloning or pulling a cycle, and using an external or cloud language model need a connection (an external server may be on your own machine or network).

## Logs and bug reports

The application writes a daily file `blah-blah-bot.log` to its log folder. Logs contain identifiers, counts and error kinds, not meeting content or secrets. When you report a problem, include the code and the message from this page, the Windows version, and the relevant lines of the log. Do not attach recordings, transcripts, voice clips, tokens or API keys. Set `BBB_LOG` (for example `debug`) to make the log more detailed.
