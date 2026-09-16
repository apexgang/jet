#!/usr/bin/env python3
"""Gate the ADR-0022 performance measurements against budgets.toml.

`check` reads the measurements `jet-daemon/tests/budgets.rs` wrote for this
host and fails when one misses its limit or regresses more than the accepted
percentage against budget-baseline.json. `accept` records the measurements
as the baseline for this host's label with a reviewed justification.
"""
import argparse
import json
import platform
import sys
import tomllib
from pathlib import Path

PACKAGES = Path(__file__).resolve().parents[2] / "packages"
CONFIG = tomllib.loads((PACKAGES / "budgets.toml").read_text())
BASELINE = PACKAGES / "budget-baseline.json"


def label():
    system = {"darwin": "macos"}.get(platform.system().lower(), platform.system().lower())
    machine = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine().lower(), platform.machine().lower())
    return f"{system}-{machine}"


def measurements_path():
    return PACKAGES / "target" / "budgets" / f"{label()}.json"


def load_measurements():
    path = measurements_path()
    if not path.exists():
        sys.exit(f"no measurements at {path}; run `just budget-test` first")
    return json.loads(path.read_text())


def load_baseline():
    if BASELINE.exists():
        return json.loads(BASELINE.read_text())
    return {}


def check(measured, accepted):
    percent = CONFIG["budget"]["regression_percent"]
    failures = []
    for name, limit in CONFIG["limits"].items():
        value = measured.get(name)
        if value is None:
            failures.append(f"{name}: not measured")
            continue
        if "max" in limit and value > limit["max"]:
            failures.append(f"{name}: {value:.3f} exceeds the {limit['max']} limit")
        if "min" in limit and value < limit["min"]:
            failures.append(f"{name}: {value:.3f} is below the {limit['min']} limit")
        base = accepted.get(name)
        if base is None:
            continue
        if "max" in limit and value > base * (1 + percent / 100):
            failures.append(f"{name}: {value:.3f} regressed over {percent}% from the accepted {base:.3f}")
        if "min" in limit and value < base * (1 - percent / 100):
            failures.append(f"{name}: {value:.3f} regressed over {percent}% from the accepted {base:.3f}")
    return failures


def report(measured, accepted):
    names = list(CONFIG["limits"]) + list(CONFIG["informational"])
    width = max(len(name) for name in names)
    for name in names:
        limit = CONFIG["limits"].get(name, {})
        bound = f"max {limit['max']}" if "max" in limit else f"min {limit['min']}" if "min" in limit else "informational"
        base = accepted.get(name)
        base = f"{base:.3f}" if base is not None else "none"
        print(f"{name:<{width}}  {measured.get(name, float('nan')):>12.3f}  {bound:<16}  accepted {base}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check", help="gate this host's measurements")
    accept = commands.add_parser("accept", help="record this host's measurements as accepted")
    accept.add_argument("--justification", required=True)
    args = parser.parse_args()

    measured = load_measurements()
    baseline = load_baseline()
    accepted = baseline.get(label(), {}).get("measurements", {})
    print(f"budgets for {label()} from {measurements_path()}")
    report(measured, accepted)
    failures = check(measured, accepted)
    if args.command == "check":
        if failures:
            sys.exit("\n".join(["budget check failed:"] + [f"  {f}" for f in failures]))
        print("budget check passed")
        return
    # A measurement over its limit is never accepted, so the drift rule
    # cannot anchor above the budget; the check keeps failing on it.
    over_limit = {f.split(":")[0] for f in failures if "limit" in f}
    for name in sorted(over_limit):
        print(f"not accepting {name}: over its limit")
    baseline[label()] = {
        "justification": args.justification,
        "measurements": {
            name: round(measured[name], 3)
            for name in list(CONFIG["limits"]) + list(CONFIG["informational"])
            if name in measured and name not in over_limit
        },
        "os": measured.get("os"),
        "arch": measured.get("arch"),
    }
    BASELINE.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n")
    print(f"accepted {label()} into {BASELINE}")


if __name__ == "__main__":
    main()
