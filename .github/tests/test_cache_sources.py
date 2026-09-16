"""Check real Cargo freshness across a checkout, including changed inputs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import cache_sources


class CargoCache(unittest.TestCase):
    def test_unchanged_checkout_is_fresh_but_changed_source_rebuilds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'src').mkdir()
            (root / 'Cargo.toml').write_text('[package]\nname="cache-smoke"\nversion="0.1.0"\nedition="2021"\n')
            source = root / 'src/main.rs'
            source.write_text('fn main() {}\n')
            migrations = root / 'migrations'
            migrations.mkdir()
            (migrations / 'first.sql').write_text('SELECT 1;\n')
            (root / 'build.rs').write_text('fn main() { println!("cargo:rerun-if-changed=migrations"); }\n')
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', 'add', 'Cargo.toml', 'src', 'build.rs', 'migrations'], cwd=root, check=True)

            def fresh():
                result = subprocess.run(['cargo', 'build', '--offline', '--message-format=json'],
                                        cwd=root, text=True, capture_output=True, check=True)
                return [item['fresh'] for line in result.stdout.splitlines()
                        if (item := json.loads(line))['reason'] == 'compiler-artifact'
                        and item['target']['name'] == 'cache-smoke']

            self.assertEqual(fresh(), [False])
            cache_sources.synchronize(root, 'save')
            source.write_text(source.read_text())
            os.utime(migrations, None)
            cache_sources.synchronize(root, 'restore')
            self.assertEqual(fresh(), [True])
            source.write_text('fn main() { println!("changed"); }\n')
            cache_sources.synchronize(root, 'restore')
            self.assertEqual(fresh(), [False])
            cache_sources.synchronize(root, 'save')
            (migrations / 'second.sql').write_text('SELECT 2;\n')
            subprocess.run(['git', 'add', 'migrations'], cwd=root, check=True)
            cache_sources.synchronize(root, 'restore')
            self.assertEqual(fresh(), [False])
            cache_sources.synchronize(root, 'save')
            subprocess.run(['git', 'rm', '-q', '-f', 'migrations/second.sql'], cwd=root, check=True)
            cache_sources.synchronize(root, 'restore')
            self.assertEqual(fresh(), [False])
