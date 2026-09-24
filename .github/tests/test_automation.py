import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import changes
import release_assets
import required

ROOT = Path(__file__).resolve().parents[2]


class ChangeSelection(unittest.TestCase):
    def test_unrelated_changes_skip_builds(self):
        self.assertEqual(changes.classify(['docs/usage.md', 'apps/jet/jet/Views/Home.swift',
                                          '.github/workflows/README.md',
                                          '.github/docs/issue-tracker.md']),
                         {'core': False, 'contracts': False, 'tauri': False})

    def test_core_does_not_rebuild_contracts(self):
        self.assertEqual(changes.classify(['packages/jet-core/src/lib.rs']),
                         {'core': True, 'contracts': False, 'tauri': False})

    def test_shared_inputs_and_deleted_files_run_gates(self):
        for path in ['packages/justfile', 'packages/Cargo.lock', '.github/scripts/release.py',
                     'packages/jet-protocol/src/removed.rs']:
            with self.subTest(path=path):
                self.assertEqual(changes.classify([path]),
                                 {'core': True, 'contracts': True, 'tauri': True})

    def test_conformance_document_is_a_test_input(self):
        self.assertTrue(changes.classify(['docs/conformance-matrix.md'])['core'])

    def test_gui_models_run_contract_tests(self):
        self.assertEqual(changes.classify(['apps/jet/jet/Protocol/JetModels.swift']),
                         {'core': False, 'contracts': True, 'tauri': False})

    def test_tauri_inputs_run_the_tauri_job(self):
        for path in ['apps/jet-tauri/src/routes/+page.svelte', 'apps/jet-tauri/bun.lock',
                     'apps/jet-tauri/src-tauri/Cargo.lock', 'fixtures/desktop/presentation-v1.json']:
            with self.subTest(path=path):
                self.assertEqual(changes.classify([path]),
                                 {'core': False, 'contracts': False, 'tauri': True})
        # The shell compiles these crates, so their changes run both suites.
        self.assertEqual(changes.classify(['packages/jet-client/src/lib.rs']),
                         {'core': True, 'contracts': False, 'tauri': True})
        self.assertEqual(changes.classify(['apps/jet-tauri/src/lib/protocol/JetModels.ts']),
                         {'core': False, 'contracts': True, 'tauri': True})


class RequiredChecks(unittest.TestCase):
    """The main ruleset requires the ubuntu-latest and macos-latest contexts (core.yml)."""

    def workflow(self):
        return (ROOT / '.github/workflows/core.yml').read_text()

    def test_gate_passes_only_when_every_selected_job_passes(self):
        self.assertEqual(required.problems('success', {'core': ('true', 'success'),
                                                       'tauri': ('false', 'skipped')}), [])
        self.assertEqual(required.problems('success', {'core': ('false', 'skipped'),
                                                       'tauri': ('true', 'success')}), [])
        for selection, jobs in [
            ('failure', {'core': ('', 'skipped'), 'tauri': ('', 'skipped')}),
            ('cancelled', {'core': ('', 'skipped'), 'tauri': ('', 'skipped')}),
            ('success', {'core': ('true', 'failure'), 'tauri': ('false', 'skipped')}),
            ('success', {'core': ('false', 'skipped'), 'tauri': ('true', 'failure')}),
            ('success', {'core': ('false', 'skipped'), 'tauri': ('true', 'cancelled')}),
            ('success', {'core': ('true', 'skipped'), 'tauri': ('false', 'skipped')}),
            ('success', {'core': ('false', 'success'), 'tauri': ('false', 'skipped')}),
            ('success', {'core': ('', ''), 'tauri': ('', '')}),
        ]:
            with self.subTest(selection=selection, jobs=jobs):
                self.assertTrue(required.problems(selection, jobs))

    def test_only_the_gate_reports_the_required_contexts(self):
        workflow = self.workflow()
        gate = workflow.split('\n  required:\n', 1)[1]
        # A job skipped by its `if` never expands its matrix, so the gate itself
        # must not have one; it runs unless the workflow was cancelled.
        self.assertEqual(workflow.count('name: ${{ matrix.os }}'), 1)
        self.assertIn('name: ${{ matrix.os }}', gate)
        self.assertIn('if: ${{ !cancelled() }}', gate)
        self.assertIn('os: [ubuntu-latest, macos-latest]', gate)
        self.assertIn(f"needs: [changes, {', '.join(required.JOBS)}]", gate)
        self.assertIn('run: python3 .github/scripts/required.py', gate)
        for job in required.JOBS:
            with self.subTest(job=job):
                self.assertIn(f'{job}: ${{{{ steps.changes.outputs.{job} }}}}', workflow)
                self.assertIn(f"if: needs.changes.outputs.{job} == 'true'", workflow)
                self.assertIn(f'{job.upper()}_SELECTED: ${{{{ needs.changes.outputs.{job} }}}}', gate)
                self.assertIn(f'{job.upper()}_RESULT: ${{{{ needs.{job}.result }}}}', gate)

    def test_tauri_job_runs_every_app_check_and_the_audit(self):
        job = self.workflow().split('\n  tauri:\n', 1)[1].split('\n  required:\n', 1)[0]
        for recipe in ('just install', 'just check', 'just audit'):
            self.assertIn(f'run: {recipe}', job)
        justfile = (ROOT / 'apps/jet-tauri/justfile').read_text()
        self.assertIn('check: contracts frontend-check frontend-test frontend-build fmt clippy test',
                      justfile)
        self.assertIn('cd ../../packages && just contracts-check', justfile)
        self.assertIn('cargo clippy --locked', justfile)
        self.assertIn('cargo test --locked', justfile)


class CodeScanning(unittest.TestCase):
    """CodeQL only sees the Swift that the manual build step compiles (codeql.yml)."""
    # Roots the Swift build step compiles: the GUI target and the wire models with their corpus runner.
    COMPILED = ('apps/jet/jet/', 'packages/jet-protocol/contracts/')
    # Xcode test bundles the build step leaves out on purpose, as the autobuilder would.
    XCODE_TESTS = ('apps/jet/jetTests/', 'apps/jet/jetUITests/')

    def test_every_swift_source_root_is_compiled_or_an_xcode_test_bundle(self):
        tracked = subprocess.check_output(['git', 'ls-files', '--', '*.swift'], cwd=ROOT, text=True).split()
        self.assertTrue(tracked)
        for path in tracked:
            with self.subTest(path=path):
                self.assertTrue(path.startswith(self.COMPILED + self.XCODE_TESTS),
                                f'{path} is outside every root the CodeQL Swift build compiles')

    def test_swift_build_step_compiles_the_known_roots(self):
        workflow = (ROOT / '.github/workflows/codeql.yml').read_text()
        self.assertIn('-project apps/jet/jet.xcodeproj -target jet', workflow)
        self.assertIn('contracts-test-swift', workflow)
        self.assertIn('build-mode: manual', workflow)


class ReleaseAssets(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.dist = Path(self.scratch.name)
        self.version = tomllib.loads((release_assets.ROOT / 'packages/Cargo.toml').read_text())['workspace']['package']['version']
        self.tag = f'v{self.version}'
        for label in release_assets.LABELS:
            self.payload(label)
            with tarfile.open(self.dist / f'jet-core-symbols-{self.version}-{label}.tar.gz', 'w:gz'):
                pass

    def payload(self, label, version=None):
        name = f'jet-core-{self.version}-{label}'
        manifest = json.dumps({'version': version or self.version, 'target': label}).encode()
        with tarfile.open(self.dist / f'{name}.tar.gz', 'w:gz') as archive:
            member = tarfile.TarInfo(f'{name}/manifest.json')
            member.size = len(manifest)
            archive.addfile(member, io.BytesIO(manifest))

    def test_formula_uses_actual_archive_checksums(self):
        release_assets.generate(self.tag, self.dist)
        formula = (self.dist / 'jet.rb').read_text()
        self.assertNotIn('@VERSION@', formula)
        for line in (self.dist / 'SHA256SUMS').read_text().splitlines():
            digest, filename = line.split('  ')
            self.assertEqual(digest, hashlib.sha256((self.dist / filename).read_bytes()).hexdigest())
            if '-symbols-' not in filename:
                self.assertIn(digest, formula)
                self.assertIn(f'/releases/download/{self.tag}/{filename}', formula)
        subprocess.run(['ruby', '-c', str(self.dist / 'jet.rb')], check=True, capture_output=True)

    def test_partial_release_cannot_generate_formula(self):
        next(self.dist.glob('*-symbols-*')).unlink()
        with self.assertRaisesRegex(ValueError, 'every target'):
            release_assets.generate(self.tag, self.dist)
        self.assertFalse((self.dist / 'jet.rb').exists())

    def test_manifest_must_match_tag(self):
        self.payload('universal-apple-darwin', version='0.0.0')
        with self.assertRaisesRegex(ValueError, 'manifest'):
            release_assets.generate(self.tag, self.dist)

    def test_invalid_and_mismatched_tags_are_rejected(self):
        for tag in ['main', 'v9999.0.0', 'v1.2.3/../../bad', 'v1.2.3";system("bad")']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release_assets.version_for(tag)


if __name__ == '__main__':
    unittest.main()
