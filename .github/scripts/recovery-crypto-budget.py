"""Enforce the age adapter's incremental 2-MiB stripped executable budget."""
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
baseline = (root / "recovery-crypto-baseline").stat().st_size
linked = (root / "recovery-crypto-budget").stat().st_size
increase = linked - baseline
limit = 2 * 1024 * 1024
print(json.dumps({"baseline_bytes": baseline, "crypto_bytes": linked,
                  "increase_bytes": increase, "limit_bytes": limit}))
if increase < 0 or increase > limit:
    raise SystemExit("Recovery cryptography exceeded its incremental size budget")
