"""Tests for check_versions.py and release_notes.py (standard library only).

Run from the repository root: python -m unittest scripts.test_release_scripts
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_versions  # noqa: E402
import release_notes  # noqa: E402

CHANGELOG = """# Changelog

## [Unreleased]

### Fixed

- Something not released yet.

## [0.5.0] - 2026-11-16

### Added

- Cycles.

## [0.4.0] - 2026-11-01

### Added

- Speakers.

## [0.3.0] - 2026-10-01

## [0.2.0] - 2026-09-01

- Old.

[Unreleased]: https://example.invalid/compare/v0.5.0...HEAD
[0.2.0]: https://example.invalid/releases/tag/v0.2.0
"""


class ReleaseNotes(unittest.TestCase):
    def test_returns_only_the_requested_section(self) -> None:
        text = release_notes.extract(CHANGELOG, "0.5.0")
        self.assertIn("### Added", text)
        self.assertIn("Cycles.", text)
        self.assertNotIn("Speakers.", text)
        self.assertNotIn("not released yet", text)

    def test_accepts_a_leading_v(self) -> None:
        self.assertEqual(release_notes.extract(CHANGELOG, "v0.4.0"), "### Added\n\n- Speakers.\n")

    def test_missing_section_fails(self) -> None:
        with self.assertRaises(ValueError):
            release_notes.extract(CHANGELOG, "9.9.9")

    def test_empty_section_fails(self) -> None:
        with self.assertRaises(ValueError):
            release_notes.extract(CHANGELOG, "0.3.0")

    def test_last_section_drops_reference_links(self) -> None:
        self.assertEqual(release_notes.extract(CHANGELOG, "0.2.0"), "- Old.\n")

    def test_a_version_is_not_matched_by_prefix(self) -> None:
        with self.assertRaises(ValueError):
            release_notes.extract(CHANGELOG, "0.5")

    def test_real_changelog_has_the_current_release(self) -> None:
        versions = check_versions.collect(check_versions.ROOT)
        current = versions["ui/package.json"]
        text = (release_notes.ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        self.assertTrue(release_notes.extract(text, current))


class Versions(unittest.TestCase):
    SAME = {"a": "1.0.0", "b": "1.0.0"}

    def test_consistent(self) -> None:
        self.assertEqual(check_versions.check(self.SAME), [])

    def test_mismatch(self) -> None:
        problems = check_versions.check({"a": "1.0.0", "b": "1.0.1"})
        self.assertTrue(any("differ" in p for p in problems))

    def test_missing_version(self) -> None:
        self.assertTrue(check_versions.check({"a": "1.0.0", "b": None}))

    def test_pre_release_is_rejected(self) -> None:
        self.assertTrue(check_versions.check({"a": "1.0.0-rc.1"}))

    def test_tag_must_match(self) -> None:
        self.assertEqual(check_versions.check(self.SAME, "v1.0.0"), [])
        self.assertTrue(check_versions.check(self.SAME, "v1.0.1"))

    def test_parsers(self) -> None:
        toml = (
            '[package]\nversion = "9.9.9"\n\n[workspace.package]\nedition = "2021"\n'
            'version = "1.2.3"\n\n[x]\nversion = "4"\n'
        )
        self.assertEqual(check_versions.cargo_workspace_version(toml), "1.2.3")
        lock = 'name = "other"\nversion = "0.1.0"\n\n[[package]]\nname = "app"\nversion = "1.2.3"\n'
        self.assertEqual(check_versions.cargo_lock_version(lock, "app"), "1.2.3")

    def test_real_repository_is_consistent(self) -> None:
        self.assertEqual(check_versions.check(check_versions.collect(check_versions.ROOT)), [])


if __name__ == "__main__":
    unittest.main()
