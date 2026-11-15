import json

import numpy as np
import pytest

from bbb_worker.steps import write_json
from bbb_worker.steps.diarize import finite_embeddings


def test_finite_embeddings_are_kept():
    rows = finite_embeddings(np.array([[0.5, -1.0], [2.0, 3.0]], dtype=np.float32))
    assert rows == [[0.5, -1.0], [2.0, 3.0]]


def test_a_voice_with_nan_or_infinite_values_gets_no_embedding():
    rows = finite_embeddings(np.array([[np.nan, np.nan], [1.0, 2.0], [np.inf, 0.0]], np.float32))
    assert rows == [[], [1.0, 2.0], []]


def test_no_voices_give_no_embeddings():
    assert finite_embeddings(np.zeros((0, 4), dtype=np.float32)) == []


def test_outputs_never_contain_nan(tmp_path):
    with pytest.raises(ValueError):
        write_json(tmp_path / "bad.json", {"embeddings": [[float("nan")]]})
    assert not (tmp_path / "bad.json").exists()
    write_json(tmp_path / "ok.json", {"embeddings": [[]]})
    assert json.loads((tmp_path / "ok.json").read_text(encoding="utf-8")) == {"embeddings": [[]]}
