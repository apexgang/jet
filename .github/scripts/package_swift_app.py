"""Bundle the universal core with the Swift app and make its release DMG/cask."""

import argparse
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path


def version_from_run(run_number: str) -> str:
    if not re.fullmatch(r"[1-9][0-9]*", run_number):
        raise ValueError("GITHUB_RUN_NUMBER must be a positive integer")
    return f"1.0.{run_number}"


def cask(version: str, digest: str) -> str:
    if not re.fullmatch(r"1\.0\.[1-9][0-9]*", version):
        raise ValueError("invalid Swift app version")
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid DMG digest")
    return f'''cask "jet" do
  version "{version}"
  sha256 "{digest}"

  url "https://github.com/apexgang/jet/releases/download/swift-v{version}/jet-app-{version}.dmg"
  name "Jet"
  desc "Native workspace for Jet coding agent conversations"
  homepage "https://github.com/apexgang/jet"

  depends_on macos: :tahoe
  depends_on formula: "apexgang/tap/jet"

  app "jet.app"
end
'''


def mac_formula(core_version: str, digest: str, app_version: str) -> str:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", core_version):
        raise ValueError("invalid core version")
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid core archive digest")
    if not re.fullmatch(r"1\.0\.[1-9][0-9]*", app_version):
        raise ValueError("invalid Swift app version")
    template = (Path(__file__).resolve().parents[1] / "packaging/homebrew/jet.rb.in").read_text()
    linux_start = template.index("  on_linux do\n")
    linux_end = template.index("  def install\n", linux_start)
    template = template[:linux_start] + template[linux_end:]
    template = template.replace('  license "Apache-2.0"\n', '  license "Apache-2.0"\n  depends_on :macos\n')
    return (template.replace("@VERSION@", core_version)
            .replace("@URL@", f"https://github.com/apexgang/jet/releases/download/swift-v{app_version}")
            .replace("@MACOS_SHA256@", digest))


def package(app: Path, core: Path, version: str, output: Path) -> Path:
    if not app.is_dir() or not core.is_dir():
        raise ValueError("built app and core payload are required")
    with (core / "manifest.json").open("rb") as file:
        manifest = json.load(file)
    if manifest.get("target") != "universal-apple-darwin":
        raise ValueError("the DMG requires a universal macOS core payload")
    for name in ("jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"):
        if not (core / name).is_file():
            raise ValueError(f"core payload is missing {name}")
        with (core / name).open("rb") as file:
            actual = hashlib.file_digest(file, "sha256").hexdigest()
        if manifest.get("executables", {}).get(name) != actual:
            raise ValueError(f"core payload has an invalid {name} digest")
    with (app / "Contents/Info.plist").open("rb") as file:
        info = plistlib.load(file)
    if info.get("CFBundleShortVersionString") != version:
        raise ValueError("app bundle version does not match the release")

    output.mkdir(parents=True, exist_ok=True)
    archive = core.parent / f"{core.name}.tar.gz"
    if not archive.is_file():
        raise ValueError("the universal core archive is missing")
    shutil.copy2(archive, output / archive.name)
    dmg = output / f"jet-app-{version}.dmg"
    with tempfile.TemporaryDirectory() as scratch:
        stage = Path(scratch)
        bundled = stage / "jet.app"
        shutil.copytree(app, bundled, symlinks=True)
        shutil.copytree(core, bundled / "Contents/Resources/jet-core")
        (stage / "Applications").symlink_to("/Applications")
        # The release has no Developer ID certificate yet. An ad-hoc signature
        # seals the finished app without changing the core manifest's hashes.
        subprocess.run(["codesign", "--force", "--sign", "-", "--options", "runtime", str(bundled)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(bundled)], check=True)
        subprocess.run(["hdiutil", "create", "-quiet", "-format", "UDZO", "-volname", "Jet",
                        "-srcfolder", str(stage), str(dmg)], check=True)

    write_metadata(dmg, output / archive.name, version, output)
    return dmg


def write_metadata(dmg: Path, core_archive: Path, version: str, output: Path) -> None:
    if dmg.name != f"jet-app-{version}.dmg":
        raise ValueError("DMG name does not match the app version")
    match = re.fullmatch(r"jet-core-([0-9]+\.[0-9]+\.[0-9]+)-universal-apple-darwin\.tar\.gz", core_archive.name)
    if match is None:
        raise ValueError("invalid core archive name")
    with tarfile.open(core_archive) as archive:
        member = archive.getmember(core_archive.name.removesuffix(".tar.gz") + "/manifest.json")
        if not member.isfile() or member.size > 65536:
            raise ValueError("invalid core archive manifest")
        manifest = json.load(archive.extractfile(member))
    if (manifest.get("version"), manifest.get("target")) != (match[1], "universal-apple-darwin"):
        raise ValueError("core archive identity does not match its name")
    with dmg.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    with core_archive.open("rb") as file:
        core_digest = hashlib.file_digest(file, "sha256").hexdigest()
    output.mkdir(parents=True, exist_ok=True)
    (output / "jet.rb").write_text(cask(version, digest))
    (output / "jet-core.rb").write_text(mac_formula(match[1], core_digest, version))
    (output / "SHA256SUMS").write_text(f"{digest}  {dmg.name}\n{core_digest}  {core_archive.name}\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path)
    parser.add_argument("--core", type=Path)
    parser.add_argument("--cask-for-dmg", type=Path)
    parser.add_argument("--core-archive", type=Path)
    parser.add_argument("--run-number", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = version_from_run(args.run_number)
    if args.cask_for_dmg:
        if args.app or args.core or not args.core_archive:
            parser.error("--cask-for-dmg requires --core-archive and cannot use --app or --core")
        write_metadata(args.cask_for_dmg, args.core_archive, version, args.output)
    elif args.app and args.core:
        print(package(args.app, args.core, version, args.output))
    else:
        parser.error("provide --app and --core, or --cask-for-dmg")
