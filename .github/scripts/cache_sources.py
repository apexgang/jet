"""Preserve Cargo freshness only for tracked inputs whose contents still match."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def synchronize(root, mode):
    cache = root / '.github/.cache/source-times.json'
    saved = json.loads(cache.read_text()) if cache.exists() else {}
    paths = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).split(b'\0')
    current = {}
    for name in paths:
        path = root / os.fsdecode(name)
        if not name or path.is_symlink() or not path.is_file():
            continue
        record = {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                  'mtime': path.stat().st_mtime_ns}
        if mode == 'restore':
            previous = saved.get(os.fsdecode(name), {})
            if previous.get('sha256') == record['sha256']:
                os.utime(path, ns=(path.stat().st_atime_ns, previous['mtime']))
            else:
                # A changed/new input must be newer than the restored artifacts.
                os.utime(path, None)
        current[os.fsdecode(name)] = record
    if mode == 'save':
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps(current))


if __name__ == '__main__':
    if len(sys.argv) != 2 or sys.argv[1] not in {'restore', 'save'}:
        raise SystemExit('expected restore or save')
    synchronize(Path(__file__).resolve().parents[2], sys.argv[1])
