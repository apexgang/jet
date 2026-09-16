#!/usr/bin/env python3
"""Build, package, and gate the Jet core executables.

Run from ``packages/`` through the justfile:

    just release-package --target <triple> [--target <triple>]
    just release-check --target <triple | universal-apple-darwin>
    just release-accept --target <...> --justification "<why>"
    just release-envelope

``package`` builds ``jetd`` with the ``release`` profile and ``jetfueld`` and
the bundled Crafts with ``release-small`` (ADR-0059), splits their symbols
into separate crash-symbol artifacts, strips them, merges two Apple targets
into one universal payload, writes the ``manifest.json`` that ``jetd core
stage`` verifies (ADR-0026), and archives payload and symbols separately.

``check`` measures every stripped executable per architecture slice against
ADR-0054, sums the payload, confirms each executable is stripped and has its
symbol artifact, enforces the per-role dependency seams of ADR-0057 and
ADR-0060 from ``cargo tree``, and compares the release profiles in
Cargo.toml with release.toml.
"""
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path

PACKAGES = Path(__file__).resolve().parents[2] / "packages"
PACKAGING = Path(__file__).resolve().parents[1] / "packaging"
CONFIG = tomllib.loads((PACKAGING / "release.toml").read_text())
BASELINE_PATH = PACKAGING / "release-baseline.json"
MIB = 1024 * 1024
UNIVERSAL = "universal-apple-darwin"
ELF_MAGIC = b"\x7fELF"
FAT_MAGICS = {b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"}


def run(args, **kwargs):
    kwargs.setdefault("check", True)
    kwargs.setdefault("text", True)
    kwargs.setdefault("cwd", PACKAGES)
    return subprocess.run(args, **kwargs)


def workspace_version():
    metadata = json.loads(
        run(["cargo", "metadata", "--no-deps", "--format-version", "1"],
            stdout=subprocess.PIPE).stdout)
    bundled = {spec["package"] for spec in CONFIG["executables"].values()}
    versions = {package["version"] for package in metadata["packages"]
                if package["name"] in bundled}
    if len(versions) != 1:
        raise SystemExit(f"bundled products must share one version, found {sorted(versions)}")
    return versions.pop()


def label_for(targets):
    if len(targets) == 1:
        return targets[0]
    if all(target.endswith("-apple-darwin") for target in targets):
        return UNIVERSAL
    raise SystemExit("only Apple targets can be merged into one universal payload")


def dist_dirs(out, label):
    version = workspace_version()
    return (out / f"jet-core-{version}-{label}",
            out / f"jet-core-symbols-{version}-{label}")


def executable_kind(path):
    with open(path, "rb") as file:
        magic = file.read(4)
    if magic == ELF_MAGIC:
        return "elf"
    if magic in FAT_MAGICS:
        return "fat"
    return "macho"


def built_path(name, target):
    profile = CONFIG["executables"][name]["profile"]
    return PACKAGES / "target" / target / profile / name


def build(targets):
    for target in targets:
        by_profile = {}
        for name, spec in CONFIG["executables"].items():
            by_profile.setdefault(spec["profile"], []).append(spec["package"])
        for profile, packages in by_profile.items():
            command = ["cargo", "build", "--locked", "--profile", profile, "--target", target]
            for package in packages:
                command += ["-p", package]
            run(command)


def split_symbols(merged, stripped, symbols, name):
    """Move debug information beside the payload and strip the executable."""
    if executable_kind(merged) == "elf":
        debug = symbols / f"{name}.debug"
        run(["objcopy", "--only-keep-debug", str(merged), str(debug)])
        run(["objcopy", "--strip-all", f"--add-gnu-debuglink={debug}",
             str(merged), str(stripped)])
    else:
        run(["dsymutil", str(merged), "-o", str(symbols / f"{name}.dSYM")])
        run(["strip", "-o", str(stripped), str(merged)])
    os.chmod(stripped, 0o755)


def describe(jetd):
    """This build's release identity, from an executable the host can run."""
    result = subprocess.run([str(jetd), "core", "describe"], text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode != 0:
        raise SystemExit(f"cannot run {jetd} to describe the release: {result.stderr}")
    return json.loads(result.stdout)


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def archive(directory):
    target = directory.parent / f"{directory.name}.tar.gz"
    with tarfile.open(target, "w:gz") as tar:
        tar.add(directory, arcname=directory.name)
    return target


def package(targets, out):
    build(targets)
    label = label_for(targets)
    payload, symbols = dist_dirs(out, label)
    for directory in (payload, symbols):
        shutil.rmtree(directory, ignore_errors=True)
        directory.mkdir(parents=True)
    with tempfile.TemporaryDirectory() as scratch:
        for name in CONFIG["executables"]:
            merged = Path(scratch) / name
            inputs = [built_path(name, target) for target in targets]
            if len(inputs) == 1:
                shutil.copy2(inputs[0], merged)
            else:
                run(["lipo", "-create", *map(str, inputs), "-output", str(merged)])
            split_symbols(merged, payload / name, symbols, name)
    identity = describe(payload / "jetd")
    manifest = {
        "version": identity["version"],
        "target": label,
        "protocol_major": identity["protocol_major"],
        "protocol_minor": identity["protocol_minor"],
        "schema_version": identity["schema_version"],
        "executables": {name: sha256(payload / name) for name in CONFIG["executables"]},
    }
    (payload / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    (payload / "SHA256SUMS").write_text("".join(
        f"{sha256(path)}  {path.name}\n" for path in sorted(payload.iterdir())
        if path.name != "SHA256SUMS"))
    archives = [archive(payload), archive(symbols)]
    print(json.dumps({"label": label, "payload": str(payload), "symbols": str(symbols),
                      "archives": [str(path) for path in archives], "manifest": manifest}))


def slices(path):
    """Size of each architecture slice of a stripped executable."""
    kind = executable_kind(path)
    if kind != "fat":
        return {arch_of(path, kind): path.stat().st_size}
    archs = run(["lipo", "-archs", str(path)], stdout=subprocess.PIPE).stdout.split()
    sizes = {}
    with tempfile.TemporaryDirectory() as scratch:
        for arch in archs:
            thin = Path(scratch) / arch
            run(["lipo", str(path), "-thin", arch, "-output", str(thin)])
            sizes[arch] = thin.stat().st_size
    return sizes


def arch_of(path, kind):
    if kind == "macho":
        return run(["lipo", "-archs", str(path)], stdout=subprocess.PIPE).stdout.strip()
    with open(path, "rb") as file:
        header = file.read(20)
    machine = int.from_bytes(header[18:20], "little" if header[5] == 1 else "big")
    return {0x3E: "x86_64", 0xB7: "aarch64"}.get(machine, f"elf-{machine:#x}")


def is_stripped(path):
    result = run(["nm", "--defined-only", str(path)], check=False,
                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    return result.stdout.strip() == ""


def has_symbols(symbols, name):
    return (symbols / f"{name}.debug").exists() or (symbols / f"{name}.dSYM").exists()


def cargo_tree(package):
    output = run(["cargo", "tree", "-p", package, "-e", "normal", "--prefix", "none",
                  "--format", "{p}"], stdout=subprocess.PIPE).stdout
    return {line.split()[0] for line in output.splitlines() if line.strip()}


def envelope():
    """Per-role dependency seams and the single pinned SQLite (ADR-0057, ADR-0060)."""
    failures = []
    for name, spec in CONFIG["executables"].items():
        names = cargo_tree(spec["package"]) - {spec["package"]}
        forbidden = sorted(names & set(spec["forbidden"]))
        if forbidden:
            failures.append(f"{name} links forbidden crates {forbidden}")
        workspace = sorted(crate for crate in names if crate.startswith("jet-"))
        unexpected = sorted(set(workspace) - set(spec["workspace"]))
        if unexpected:
            failures.append(f"{name} links workspace crates outside its seam {unexpected}")
    sqlite = CONFIG["sqlite"]
    lock = (PACKAGES / "Cargo.lock").read_text()
    pinned = lock.count(f'name = "{sqlite["crate"]}"')
    if pinned != 1:
        failures.append(f"{sqlite['crate']} must be pinned once in Cargo.lock, found {pinned}")
    dependents = run(["cargo", "tree", "-i", sqlite["crate"], "-e", "normal", "--workspace",
                      "--prefix", "none", "--format", "{p}"], stdout=subprocess.PIPE).stdout
    linking = sorted({line.split()[0] for line in dependents.splitlines()
                      if line.startswith("jet-")})
    if linking != sorted(sqlite["dependents"]):
        failures.append(f"{sqlite['crate']} is reached by {linking}, expected {sorted(sqlite['dependents'])}")
    return failures


def profiles():
    """Release profiles in Cargo.toml must say what release.toml accepts (ADR-0059)."""
    actual = tomllib.loads((PACKAGES / "Cargo.toml").read_text()).get("profile", {})
    failures = []
    for profile, expected in CONFIG["profiles"].items():
        for key, value in expected.items():
            found = actual.get(profile, {}).get(key)
            if found != value:
                failures.append(f"profile.{profile}.{key} is {found!r}, ADR-0059 requires {value!r}")
    return failures


def measure(label, out):
    payload, symbols = dist_dirs(out, label)
    if not payload.is_dir():
        raise SystemExit(f"no payload at {payload}; run `just release-package` first")
    report = {}
    for name in CONFIG["executables"]:
        path = payload / name
        report[name] = {"slices": slices(path), "stripped": is_stripped(path),
                        "symbols": has_symbols(symbols, name)}
    return report


def check(label, out):
    failures = envelope() + profiles()
    report = measure(label, out)
    baseline = json.loads(BASELINE_PATH.read_text()).get(label)
    percent = CONFIG["budget"]["increase_percent"]
    per_arch = {}
    for name, entry in report.items():
        limit = CONFIG["executables"][name]["limit_mib"] * MIB
        if not entry["stripped"]:
            failures.append(f"{name} still carries symbols")
        if not entry["symbols"]:
            failures.append(f"{name} has no crash-symbol artifact")
        for arch, size in entry["slices"].items():
            per_arch[arch] = per_arch.get(arch, 0) + size
            if size > limit:
                failures.append(f"{name} ({arch}) is {size / MIB:.2f} MiB, over its {limit // MIB} MiB budget")
            accepted = (baseline or {}).get("executables", {}).get(name, {}).get(arch)
            if accepted is None:
                print(f"note: no accepted size for {name} ({arch}) on {label}; only the budget applies",
                      file=sys.stderr)
            elif size > accepted * (1 + percent / 100):
                failures.append(f"{name} ({arch}) grew from {accepted / MIB:.2f} to {size / MIB:.2f} MiB, "
                                f"over {percent}% without `just release-accept`")
    cap = CONFIG["budget"]["payload_mib"] * MIB
    for arch, total in per_arch.items():
        if total > cap:
            failures.append(f"the {arch} payload is {total / MIB:.2f} MiB, over its {cap // MIB} MiB cap")
    print(json.dumps({"label": label, "executables": report, "payload_bytes": per_arch,
                      "failures": failures}, indent=2))
    if failures:
        raise SystemExit("release check failed:\n- " + "\n- ".join(failures))


def accept(label, out, justification):
    """Record measured sizes as accepted. A slice over its budget is never
    accepted: the baseline governs drift, the budget governs the limit."""
    report = measure(label, out)
    accepted = {}
    for name, entry in report.items():
        limit = CONFIG["executables"][name]["limit_mib"] * MIB
        within = {arch: size for arch, size in entry["slices"].items() if size <= limit}
        for arch in entry["slices"].keys() - within.keys():
            print(f"note: {name} ({arch}) is over its budget and is not accepted", file=sys.stderr)
        if within:
            accepted[name] = within
    baseline = json.loads(BASELINE_PATH.read_text())
    baseline[label] = {
        "version": workspace_version(),
        "justification": justification,
        "executables": accepted,
    }
    BASELINE_PATH.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n")
    print(json.dumps(baseline[label], indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("package", "check", "accept"):
        command = commands.add_parser(name)
        command.add_argument("--target", action="append", required=True)
        command.add_argument("--out", type=Path, default=PACKAGES / "target" / "dist")
        if name == "accept":
            command.add_argument("--justification", required=True)
    commands.add_parser("envelope")
    args = parser.parse_args()
    if args.command == "package":
        package(args.target, args.out)
    elif args.command == "check":
        check(label_for(args.target) if len(args.target) > 1 else args.target[0], args.out)
    elif args.command == "accept":
        accept(label_for(args.target) if len(args.target) > 1 else args.target[0], args.out,
               args.justification)
    else:
        failures = envelope() + profiles()
        print(json.dumps({"failures": failures}, indent=2))
        if failures:
            raise SystemExit(1)


if __name__ == "__main__":
    main()
