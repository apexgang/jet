"""Exercise the platform's real strip, symbol, and signing tools on a tiny binary."""
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import release


class NativePackaging(unittest.TestCase):
    def test_stripped_payload_keeps_separate_symbols_and_runs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'smoke.c'
            source.write_text('#include <stdio.h>\nint main(void) { puts("packaging smoke"); return 0; }\n')
            merged = root / 'merged'
            if platform.system() == 'Darwin':
                binaries = []
                for arch in ('arm64', 'x86_64'):
                    obj, binary = root / f'{arch}.o', root / arch
                    subprocess.run(['cc', '-g', '-arch', arch, '-c', str(source), '-o', str(obj)], check=True)
                    subprocess.run(['cc', '-arch', arch, str(obj), '-o', str(binary)], check=True)
                    binaries.append(str(binary))
                subprocess.run(['lipo', '-create', *binaries, '-output', str(merged)], check=True)
            else:
                subprocess.run(['cc', '-g', str(source), '-o', str(merged)], check=True)
            symbols = root / 'symbols'
            symbols.mkdir()
            payload = root / 'jetd'
            self.assertFalse(release.is_stripped(merged))
            release.split_symbols(merged, payload, symbols, 'jetd')
            self.assertTrue(release.is_stripped(payload))
            self.assertTrue(release.has_symbols(symbols, 'jetd'))
            if platform.system() == 'Darwin':
                subprocess.run(['codesign', '--verify', '--strict', '--all-architectures', str(payload)], check=True)
                self.assertEqual(set(release.slices(payload)), {'arm64', 'x86_64'})
            self.assertEqual(subprocess.check_output([str(payload)], text=True), 'packaging smoke\n')
