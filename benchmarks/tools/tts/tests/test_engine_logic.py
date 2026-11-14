import json

import pytest

from bbb_tts import cli
from bbb_tts.engine import plausible


def test_plausible_accepts_normal_speech():
    assert plausible("Dzień dobry wszystkim, zaczynamy spotkanie.", 3.0)


def test_plausible_rejects_silence_and_babble():
    text = "Zgoda."
    assert not plausible(text, 0.01)
    assert not plausible(text, 30.0)


def test_load_script_validates(tmp_path):
    good = tmp_path / "ok.json"
    good.write_text(
        json.dumps({"speakers": ["a"], "turns": [{"speaker": "a", "lang": "pl", "text": "x"}]})
    )
    assert cli.load_script(good)["speakers"] == ["a"]
    bad = tmp_path / "bad.json"
    bad.write_text(json.dumps({"speakers": ["a"], "turns": [{"speaker": "a"}]}))
    with pytest.raises(ValueError):
        cli.load_script(bad)
