"""Every model is pinned to one commit, and the pin reaches download and cache lookup."""

import re
import sys
import types

import pytest

from bbb_worker import models

HEX40 = re.compile(r"[0-9a-f]{40}")


@pytest.mark.parametrize("key", sorted(models.MODELS))
def test_every_configured_model_has_a_full_commit_hash(key):
    assert HEX40.fullmatch(models.MODELS[key].revision)


def test_download_and_local_lookup_pass_the_pinned_revision(monkeypatch, tmp_path):
    calls = []

    def fake_snapshot_download(repo, **kwargs):
        calls.append((repo, kwargs))
        raise FileNotFoundError("stop here")

    hub = types.ModuleType("huggingface_hub")
    hub.snapshot_download = fake_snapshot_download
    tqdm_auto = types.ModuleType("tqdm.auto")
    tqdm_auto.tqdm = type("tqdm", (), {})
    monkeypatch.setitem(sys.modules, "huggingface_hub", hub)
    monkeypatch.setitem(sys.modules, "tqdm.auto", tqdm_auto)
    monkeypatch.delenv("BBB_VISION_DIR", raising=False)
    monkeypatch.setenv("BBB_MODELS_DIR", str(tmp_path))
    for key, spec in models.MODELS.items():
        calls.clear()
        with pytest.raises(models.ModelMissing):
            models.local_path(key)
        assert calls and calls[0][1]["revision"] == spec.revision
        calls.clear()
        with pytest.raises(FileNotFoundError):
            models.download(key, token="test-token")
        assert calls and calls[0][1]["revision"] == spec.revision
