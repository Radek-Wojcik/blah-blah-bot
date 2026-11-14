import json

from bbb_bench import harness


def test_write_result_embeds_environment(tmp_path):
    path = harness.write_result("p0", "unit", {"metric": 1.5}, out_dir=tmp_path)
    doc = json.loads(path.read_text(encoding="utf-8"))
    assert path.name == "p0-unit.json"
    assert doc["metric"] == 1.5
    assert "python" in doc["environment"]


def test_run_prototype_records_failure(tmp_path, monkeypatch):
    monkeypatch.setattr(harness, "RESULTS_DIR", tmp_path)

    def boom():
        raise RuntimeError("no model")

    path = harness.run_prototype("p0", "fail", boom)
    doc = json.loads(path.read_text(encoding="utf-8"))
    assert doc["status"] == "error"
    assert "no model" in doc["error"]


def test_timer_measures_elapsed():
    with harness.timer() as elapsed:
        pass
    assert elapsed() >= 0
