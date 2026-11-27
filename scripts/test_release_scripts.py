"""Tests for release scripts and installer guard wiring (standard library only).

Run from the repository root: python -m unittest scripts.test_release_scripts
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess



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


import json
import os
import re
import shutil
import subprocess
class BundleGuard(unittest.TestCase):
    def test_build_installer_sets_and_restores_installer_build_flag(self) -> None:
        script = (check_versions.ROOT / "scripts" / "build-installer.ps1").read_text(encoding="utf-8")
        self.assertRegex(script, r'\$env:BBB_INSTALLER_BUILD\s*=\s*"1"')
        self.assertRegex(
            script,
            r"finally\s*\{[^}]*\$env:BBB_INSTALLER_BUILD\s*=\s*(\$null|\$saved\w+)",
        )

    def test_build_installer_invokes_tauri_build_with_bundle_config(self) -> None:
        script = (check_versions.ROOT / "scripts" / "build-installer.ps1").read_text(encoding="utf-8")
        self.assertIn("tauri.bundle.conf.json", script)
        self.assertRegex(script, r"&\s*\$tauri\s+build\b.*@configs")

    def test_build_rs_enforces_guard_on_tauri_config_and_installer_build(self) -> None:
        build_rs = (check_versions.ROOT / "crates" / "app" / "build.rs").read_text(encoding="utf-8")
        self.assertIn("TAURI_CONFIG", build_rs)
        self.assertIn("BBB_INSTALLER_BUILD", build_rs)
        self.assertIn("should_enforce", build_rs)
        self.assertIn("check_bundle_resources", build_rs)
        self.assertIsNotNone(
            re.search(r'env::var\("TAURI_CONFIG"\)', build_rs),
            "build.rs must read TAURI_CONFIG",
        )
        self.assertIsNotNone(
            re.search(r'env::var\("BBB_INSTALLER_BUILD"\)', build_rs),
            "build.rs must read BBB_INSTALLER_BUILD",
        )


class SigningHookContract(unittest.TestCase):
    SCRIPT = Path(__file__).resolve().parent / "build-installer.ps1"

    def test_validates_sign_command_and_writes_utf8_no_bom(self) -> None:
        text = self.SCRIPT.read_text(encoding="utf-8")
        self.assertIn("BBB_SIGN_COMMAND", text)
        self.assertIn("Resolve-SignCommand", text)
        self.assertIn("%1", text)
        self.assertIn("UTF8Encoding($false)", text)
        self.assertNotRegex(text, r"Set-Content\s+-LiteralPath\s+\$signConfig.*-Encoding\s+utf8")

    EXTRACT = (
        "$ErrorActionPreference = 'Stop'; "
        "$ast = [System.Management.Automation.Language.Parser]::ParseFile("
        "$env:BBB_TEST_SCRIPT, [ref]$null, [ref]$null); "
        "$fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language."
        "FunctionDefinitionAst] -and $n.Name -eq 'Resolve-SignCommand' }, $true); "
        "Invoke-Expression $fn.Extent.Text; "
    )

    def resolve(self, raw: str) -> subprocess.CompletedProcess[str]:
        """Run the real Resolve-SignCommand on fake input; nothing is signed or written."""
        powershell = shutil.which("powershell") or shutil.which("pwsh")
        if powershell is None:
            self.skipTest("PowerShell is not available")
        env = dict(os.environ, BBB_TEST_SCRIPT=str(self.SCRIPT), BBB_TEST_INPUT=raw)
        command = self.EXTRACT + (
            "$r = Resolve-SignCommand $env:BBB_TEST_INPUT; "
            "if ($r -is [string]) { 'string' } else { 'object' }"
        )
        return subprocess.run(
            [powershell, "-NoProfile", "-NonInteractive", "-Command", command],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
            timeout=120,
            check=False,
        )

    def test_resolve_sign_command_accepts_valid_input(self) -> None:
        plain = self.resolve("signtool sign /fd sha256 %1")
        self.assertEqual(plain.returncode, 0, plain.stderr)
        self.assertEqual(plain.stdout.strip(), "string")
        structured = self.resolve('{"cmd":"signtool","args":["sign","%1"]}')
        self.assertEqual(structured.returncode, 0, structured.stderr)
        self.assertEqual(structured.stdout.strip(), "object")

    def test_resolve_sign_command_rejects_bad_input(self) -> None:
        bad = [
            "signtool sign",  # no placeholder
            '{"cmd":"signtool","args":["sign"]}',  # no placeholder in args
            '{"cmd":"signtool","args":"sign %1"}',  # scalar args
            '{"cmd":"","args":["%1"]}',  # empty cmd
            '{"cmd":"signtool"',  # broken JSON
        ]
        for raw in bad:
            with self.subTest(raw=raw):
                result = self.resolve(raw)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("BBB_SIGN_COMMAND", result.stderr)

    def test_verifies_signature_and_supports_defender_scan(self) -> None:
        text = self.SCRIPT.read_text(encoding="utf-8")
        self.assertIn("Get-AuthenticodeSignature", text)
        self.assertIn("[switch]$Scan", text)
        self.assertIn("MpCmdRun.exe", text)
        self.assertIn("-DisableRemediation", text)


class InstallerMetadata(unittest.TestCase):
    APP_DIR = Path(__file__).resolve().parent.parent / "crates" / "app"

    def test_tauri_conf_installer_metadata(self) -> None:
        conf = json.loads((self.APP_DIR / "tauri.conf.json").read_text(encoding="utf-8"))
        bundle = conf["bundle"]
        self.assertEqual(bundle["homepage"], "https://github.com/Radek-Wojcik/blah-blah-bot")
        windows = bundle["windows"]
        self.assertEqual(windows["webviewInstallMode"], {"type": "skip"})
        nsis = windows["nsis"]
        self.assertEqual(nsis["compression"], "zlib")
        self.assertEqual(nsis["installerIcon"], "icons/icon.ico")
        self.assertEqual(nsis["uninstallerIcon"], "icons/icon.ico")
        self.assertEqual(nsis["template"], "nsis/installer.nsi")
        self.assertEqual(nsis["installerHooks"], "nsis/hooks.nsh")

    def test_cargo_winres_metadata_is_ascii(self) -> None:
        cargo = (self.APP_DIR / "Cargo.toml").read_text(encoding="utf-8")
        match = re.search(r"\[package\.metadata\.tauri-winres\]\s*(.*?)(?:\n\[|\Z)", cargo, re.S)
        self.assertIsNotNone(match, "missing [package.metadata.tauri-winres] section")
        section = match.group(1)
        self.assertIn('OriginalFilename = "app.exe"', section)
        self.assertIn('InternalName = "app"', section)
        self.assertIn("Comments =", section)
        self.assertTrue(section.isascii(), "tauri-winres metadata must be ASCII")

    def test_nsis_hooks_and_template(self) -> None:
        hooks = (self.APP_DIR / "nsis" / "hooks.nsh").read_text(encoding="utf-8")
        for key in ("CompanyName", "InternalName", "OriginalFilename", "Comments"):
            self.assertIn(f'VIAddVersionKey "{key}"', hooks)
        self.assertIn("Function VerifyWritableInstDir", hooks)
        self.assertIn("!macro NSIS_HOOK_PREINSTALL", hooks)
        self.assertIn("Call VerifyWritableInstDir", hooks)
        self.assertIn("!macro NSIS_HOOK_POSTUNINSTALL", hooks)
        self.assertIn("${If} $UpdateMode <> 1", hooks.split("NSIS_HOOK_POSTUNINSTALL")[1])
        self.assertIn('DeleteRegValue SHCTX "${MANUPRODUCTKEY}" ""', hooks)
        self.assertIn('DeleteRegKey /ifempty SHCTX "${MANUPRODUCTKEY}"', hooks)
        self.assertIn('DeleteRegKey /ifempty SHCTX "${MANUKEY}"', hooks)
        self.assertNotIn("HKCU", hooks)

        template = (self.APP_DIR / "nsis" / "installer.nsi").read_text(encoding="utf-8")
        self.assertRegex(
            template,
            r"!define\s+MUI_PAGE_CUSTOMFUNCTION_LEAVE\s+VerifyWritableInstDir\s*\r?\n!insertmacro\s+MUI_PAGE_DIRECTORY",
        )


if __name__ == "__main__":
    unittest.main()
