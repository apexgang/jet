"""Check Swift app release metadata and the standalone DMG contents."""

import hashlib
import json
import os
import plistlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
import package_swift_app


class SwiftRelease(unittest.TestCase):
    def test_versions_and_cask_dependency(self):
        self.assertEqual(package_swift_app.version_from_run("42"), "1.0.42")
        for bad in ("0", "-1", "1.0", "$(id)"):
            with self.assertRaises(ValueError):
                package_swift_app.version_from_run(bad)
        source = package_swift_app.cask("1.0.42", "a" * 64)
        self.assertIn('depends_on formula: "apexgang/tap/jet"', source)
        self.assertIn("swift-v1.0.42/jet-app-1.0.42.dmg", source)

    def test_dmg_contains_core_and_uses_the_dmg_checksum(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "built/jet.app"
            (app / "Contents/Resources").mkdir(parents=True)
            with (app / "Contents/Info.plist").open("wb") as file:
                plistlib.dump({"CFBundleShortVersionString": "1.0.42"}, file)
            core = root / "jet-core-0.2.0-universal-apple-darwin"
            core.mkdir()
            digests = {}
            for name in ("jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"):
                binary = name.encode()
                (core / name).write_bytes(binary)
                digests[name] = hashlib.sha256(binary).hexdigest()
            (core / "manifest.json").write_text(json.dumps({
                "version": "0.2.0", "target": "universal-apple-darwin",
                "executables": digests,
            }))
            archive = root / "jet-core-0.2.0-universal-apple-darwin.tar.gz"
            with tarfile.open(archive, "w:gz") as file:
                file.add(core, arcname="jet-core-0.2.0-universal-apple-darwin")

            def command(args, check):
                self.assertTrue(check)
                if args[0] == "codesign" and "--force" in args:
                    bundled = Path(args[-1])
                    self.assertEqual((bundled / "Contents/Resources/jet-core/jetd").read_bytes(), b"jetd")
                    self.assertEqual((bundled.parent / "Applications").readlink(), Path("/Applications"))
                if args[0] == "hdiutil":
                    Path(args[-1]).write_bytes(b"test dmg")

            with patch.object(package_swift_app.subprocess, "run", side_effect=command):
                dmg = package_swift_app.package(app, core, "1.0.42", root / "dist")
            digest = hashlib.sha256(b"test dmg").hexdigest()
            self.assertIn(f"{digest}  {dmg.name}\n", (root / "dist/SHA256SUMS").read_text())
            self.assertIn(f'sha256 "{digest}"', (root / "dist/jet.rb").read_text())
            self.assertIn('version "0.2.0"', (root / "dist/jet-core.rb").read_text())
            self.assertIn("depends_on :macos", (root / "dist/jet-core.rb").read_text())
            self.assertNotIn("on_linux do", (root / "dist/jet-core.rb").read_text())

            (core / "jetd").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "invalid jetd digest"):
                package_swift_app.package(app, core, "1.0.42", root / "other")

    def test_tap_publishes_formula_and_cask_without_downgrading(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tap = root / "homebrew-tap"
            tap.mkdir()
            (root / "swift-dist").mkdir()
            tools = root / "bin"
            tools.mkdir()
            gh = tools / "gh"
            gh.write_text("#!/bin/sh\nprintf '12345\\n'\n")
            gh.chmod(0o755)
            env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                       GIT_CONFIG_SYSTEM=os.devnull,
                       PATH=f"{tools}{os.pathsep}{os.environ['PATH']}")

            def git(*args):
                return subprocess.check_output(["git", *args], cwd=tap, env=env,
                                               text=True, stderr=subprocess.PIPE).strip()

            git("init", "--bare", "--initial-branch=main", str(root / "remote"))
            git("init", "--initial-branch=main")
            git("config", "user.name", "Test")
            git("config", "user.email", "test@example.invalid")
            git("commit", "--allow-empty", "-m", "Initialized tap")
            git("remote", "add", "origin", str(root / "remote"))
            git("push", "origin", "main")

            def publish(app_version, core_version):
                (root / "swift-dist/jet.rb").write_text(package_swift_app.cask(app_version, "a" * 64))
                (root / "swift-dist/jet-core.rb").write_text(
                    package_swift_app.mac_formula(core_version, "b" * 64, app_version))
                subprocess.run(["bash", str(SCRIPTS / "publish-swift-cask.sh"), app_version],
                               cwd=root, env=env, check=True, capture_output=True, text=True)
                return git("rev-parse", "HEAD")

            first = publish("1.0.42", "0.2.0")
            self.assertTrue((tap / "Formula/jet.rb").is_file())
            self.assertTrue((tap / "Casks/jet.rb").is_file())
            self.assertEqual(publish("1.0.42", "0.2.0"), first)
            self.assertEqual(publish("1.0.41", "0.1.0"), first)
            self.assertNotEqual(publish("1.0.43", "0.3.0"), first)
            self.assertIn('version "0.3.0"', (tap / "Formula/jet.rb").read_text())
            self.assertIn('version "1.0.43"', (tap / "Casks/jet.rb").read_text())


if __name__ == "__main__":
    unittest.main()
