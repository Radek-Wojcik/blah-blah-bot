"""First-run errors name the problem and the fix instead of a raw operating-system error."""

import sys
import types

import pytest

from bbb_worker import llama, models, schema

from .test_contracts import fixture


def test_a_missing_llama_server_gives_an_actionable_message(monkeypatch, tmp_path):
    missing = tmp_path / "llama-server.exe"
    monkeypatch.setenv("BBB_LLAMA_SERVER", str(missing))
    with pytest.raises(RuntimeError) as e:
        with llama.running(tmp_path / "model.gguf", startup_timeout_s=1):
            pass
    message = str(e.value)
    assert f"llama-server was not found at '{missing}'" in message
    assert "Settings > Processing" in message and "setup" in message


def test_a_bare_llama_server_name_is_reported_as_missing_from_the_path():
    message = llama.not_found_message("llama-server")
    assert "was not found on the PATH" in message
    assert "Settings > Processing" in message


def test_a_llama_server_that_cannot_start_is_not_reported_as_missing(monkeypatch, tmp_path):
    # A folder is found but cannot be run: the message says so and does not claim it is missing.
    monkeypatch.setenv("BBB_LLAMA_SERVER", str(tmp_path))
    with pytest.raises(RuntimeError) as e:
        with llama.running(tmp_path / "model.gguf", startup_timeout_s=1):
            pass
    assert "could not be started" in str(e.value)
    assert "was not found" not in str(e.value)


def test_missing_model_messages_point_to_the_settings_and_the_setup(monkeypatch, tmp_path):
    # The CPU-only test environment has no model libraries: stand in for the empty local cache.
    def not_cached(*args, **kwargs):
        raise FileNotFoundError("not in the local cache")

    hub = types.ModuleType("huggingface_hub")
    hub.snapshot_download = not_cached
    tqdm_auto = types.ModuleType("tqdm.auto")
    tqdm_auto.tqdm = type("tqdm", (), {})
    monkeypatch.setitem(sys.modules, "huggingface_hub", hub)
    monkeypatch.setitem(sys.modules, "tqdm.auto", tqdm_auto)
    monkeypatch.delenv("BBB_VISION_DIR", raising=False)
    monkeypatch.setenv("BBB_MODELS_DIR", str(tmp_path))
    with pytest.raises(models.ModelMissing) as e:
        models.local_path("vision")
    assert "Settings > Processing" in str(e.value) and "setup" in str(e.value)
    with pytest.raises(models.ModelMissing) as e:
        models.download("diarization", token=None)
    assert "setup" in str(e.value)


def test_the_manifest_accepts_the_skip_vision_option():
    doc = fixture("manifest.json")
    doc["options"]["skip_vision"] = True
    schema.validate("job-manifest", doc)
    doc["options"]["skip_vision"] = "yes"
    with pytest.raises(Exception):
        schema.validate("job-manifest", doc)
