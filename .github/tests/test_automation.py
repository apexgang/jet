import base64
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

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
        check = re.search(r'(?m)^check: (.*)$', justfile).group(1).split()
        for recipe in ('version-check', 'contracts', 'frontend-check', 'frontend-test', 'frontend-build',
                       'fmt', 'clippy', 'test', 'e2e-dry-run'):
            self.assertIn(recipe, check)
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


def public_key(key_id):
    """A `tauri signer` public key: base64 of a minisign key file."""
    line = base64.b64encode(b'Ed' + key_id + bytes(32)).decode()
    text = f'untrusted comment: minisign public key: {key_id[::-1].hex().upper()}\n{line}\n'
    return base64.b64encode(text.encode()).decode()


def signature_file(bundle, version, key_id):
    """What Tauri writes to `<bundle>.sig`: base64 of a minisign signature box."""
    signature = base64.b64encode(b'ED' + key_id + bytes(64)).decode()
    box = (f'untrusted comment: signature from tauri secret key\n{signature}\n'
           f'trusted comment: timestamp:1790000000\tfile:{bundle}\tversion:{version}\n'
           f'{base64.b64encode(bytes(64)).decode()}\n')
    return base64.b64encode(box.encode())


class ReleaseAssets(unittest.TestCase):
    """release_assets.py against a scratch repository that carries the real Homebrew templates."""
    VERSION = '0.2.0'
    KEY = bytes.fromhex('315b129c8ea330b5')

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name) / 'repository'
        self.write('packages/Cargo.toml', f'[workspace.package]\nversion = "{self.VERSION}"\n')
        self.app_versions(self.VERSION)
        self.write('apps/jet-tauri/src-tauri/tauri.release.conf.json',
                   json.dumps({'plugins': {'updater': {'pubkey': public_key(self.KEY)}}}))
        shutil.copytree(ROOT / '.github/packaging/homebrew', self.root / '.github/packaging/homebrew')
        patcher = mock.patch.object(release_assets, 'ROOT', self.root)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.tag = f'v{self.VERSION}'
        self.dist = Path(scratch.name) / 'dist'
        self.desktop = Path(scratch.name) / 'desktop'
        self.dist.mkdir()
        self.desktop.mkdir()
        for label in release_assets.LABELS:
            self.payload(label)
            with tarfile.open(self.dist / f'jet-core-symbols-{self.VERSION}-{label}.tar.gz', 'w:gz'):
                pass
        for label in release_assets.DESKTOP:
            self.bundles(label)

    def write(self, path, text):
        (self.root / path).parent.mkdir(parents=True, exist_ok=True)
        (self.root / path).write_text(text)

    def app_versions(self, version, cargo=None):
        self.write('apps/jet-tauri/src-tauri/tauri.conf.json', json.dumps({'productName': 'Jet', 'version': version}))
        self.write('apps/jet-tauri/src-tauri/Cargo.toml',
                   f'[package]\nname = "jet-tauri"\nversion = "{cargo or version}"\n')
        self.write('apps/jet-tauri/package.json', json.dumps({'name': 'jet-tauri', 'version': version}))

    def payload(self, label, version=None, dist=None):
        name = f'jet-core-{self.VERSION}-{label}'
        manifest = json.dumps({'version': version or self.VERSION, 'target': label}).encode()
        with tarfile.open((dist or self.dist) / f'{name}.tar.gz', 'w:gz') as archive:
            member = tarfile.TarInfo(f'{name}/manifest.json')
            member.size = len(manifest)
            archive.addfile(member, io.BytesIO(manifest))

    def bundles(self, label, desktop=None, signed=True):
        desktop = desktop or self.desktop
        for name in release_assets.DESKTOP[label][1].values():
            bundle = name.format(version=self.VERSION)
            (desktop / bundle).write_bytes(f'{bundle} contents'.encode())
            if signed:
                (desktop / f'{bundle}.sig').write_bytes(signature_file(bundle, self.VERSION, self.KEY))

    def generate(self, pub_date='2026-09-24T12:00:00+03:00'):
        release_assets.generate(self.tag, self.dist, self.desktop, pub_date)

    def test_every_published_asset_is_checksummed_and_rendered_from_its_bytes(self):
        self.generate()
        published = ({p.name: p for p in self.dist.glob('*.tar.gz')}
                     | {p.name: p for p in self.desktop.iterdir()}
                     | {name: self.dist / name for name in ('jetd.rb', 'jet-app.rb', 'latest.json')})
        sums = dict(reversed(line.split('  ')) for line in (self.dist / 'SHA256SUMS').read_text().splitlines())
        self.assertEqual(set(sums), set(published))
        self.assertEqual(len(sums), 6 + 12 + 3)
        for name, path in published.items():
            with self.subTest(asset=name):
                self.assertEqual(sums[name], hashlib.sha256(path.read_bytes()).hexdigest())
        formula = (self.dist / 'jetd.rb').read_text()
        cask = (self.dist / 'jet-app.rb').read_text()
        self.assertNotRegex(formula + cask, '@[A-Z_0-9]+@')
        self.assertNotIn(release_assets.UNBUILT, formula + cask)
        base = f'https://github.com/apexgang/jet/releases/download/{self.tag}'
        for label in release_assets.LABELS:
            name = f'jet-core-{self.VERSION}-{label}.tar.gz'
            self.assertIn(f'url "{base}/{name}"', formula)
            self.assertIn(sums[name], formula)
        self.assertIn(f'url "{base}/Jet_#{{version}}_#{{arch}}.AppImage"', cask)
        self.assertIn(f'arm64_linux:  "{sums[f"Jet_{self.VERSION}_aarch64.AppImage"]}"', cask)
        self.assertIn(f'x86_64_linux: "{sums[f"Jet_{self.VERSION}_amd64.AppImage"]}"', cask)
        self.assertIn(f'version "{self.VERSION}"', cask)
        for rendered in ('jetd.rb', 'jet-app.rb'):
            subprocess.run(['ruby', '-c', str(self.dist / rendered)], check=True, capture_output=True)

    def test_updater_manifest_carries_signature_contents_not_paths(self):
        self.generate()
        manifest = json.loads((self.dist / 'latest.json').read_text())
        self.assertEqual(manifest['version'], self.VERSION)
        # The tagged commit's date, in UTC, never the build's clock.
        self.assertEqual(manifest['pub_date'], '2026-09-24T09:00:00Z')
        base = f'https://github.com/apexgang/jet/releases/download/{self.tag}'
        expected = {
            'linux-x86_64-appimage': f'Jet_{self.VERSION}_amd64.AppImage',
            'linux-x86_64-deb': f'Jet_{self.VERSION}_amd64.deb',
            'linux-x86_64-rpm': f'Jet-{self.VERSION}-1.x86_64.rpm',
            'linux-aarch64-appimage': f'Jet_{self.VERSION}_aarch64.AppImage',
            'linux-aarch64-deb': f'Jet_{self.VERSION}_arm64.deb',
            'linux-aarch64-rpm': f'Jet-{self.VERSION}-1.aarch64.rpm',
        }
        self.assertEqual(set(manifest['platforms']), set(expected))
        for platform, bundle in expected.items():
            with self.subTest(platform=platform):
                entry = manifest['platforms'][platform]
                self.assertEqual(entry, {'url': f'{base}/{bundle}',
                                         'signature': (self.desktop / f'{bundle}.sig').read_text()})

    def test_incomplete_or_extra_assets_cannot_generate_a_release(self):
        cases = {
            'every target': lambda: next(self.dist.glob('*-symbols-*')).unlink(),
            'Linux target': lambda: next(self.desktop.glob('*.rpm')).unlink(),
            'signatures': lambda: next(self.desktop.glob('*.deb.sig')).unlink(),
            'bundles': lambda: (self.desktop / f'Jet_{self.VERSION}_amd64.AppImage.tar.gz').write_bytes(b''),
        }
        for message, damage in cases.items():
            with self.subTest(damage=message):
                self.setUp()
                damage()
                with self.assertRaisesRegex(ValueError, message):
                    self.generate()
                self.assertFalse((self.dist / 'jetd.rb').exists())
                self.assertFalse((self.dist / 'latest.json').exists())

    def test_manifest_must_match_tag(self):
        self.payload('universal-apple-darwin', version='0.0.0')
        with self.assertRaisesRegex(ValueError, 'manifest'):
            self.generate()

    def test_signatures_must_sign_the_named_bundle_with_the_updater_key(self):
        bundle = f'Jet_{self.VERSION}_amd64.AppImage'
        other = f'Jet_{self.VERSION}_amd64.deb'
        cases = {
            'a path': str(self.desktop / f'{bundle}.sig').encode(),
            'a trailing newline': signature_file(bundle, self.VERSION, self.KEY) + b'\n',
            'another bundle': signature_file(other, self.VERSION, self.KEY),
            'another version': signature_file(bundle, '0.1.0', self.KEY),
            'another key': signature_file(bundle, self.VERSION, bytes(8)),
        }
        for case, content in cases.items():
            with self.subTest(signature=case):
                (self.desktop / f'{bundle}.sig').write_bytes(content)
                with self.assertRaisesRegex(ValueError, f'{bundle}.sig'):
                    self.generate()
                self.assertFalse((self.dist / 'latest.json').exists())

    def test_release_configuration_must_pin_the_updater_key(self):
        config = self.root / 'apps/jet-tauri/src-tauri/tauri.release.conf.json'
        for text in ['{}', '{"plugins": {"updater": {"pubkey": "not base64!"}}}', None]:
            with self.subTest(config=text):
                if text is None:
                    config.unlink()
                else:
                    config.write_text(text)
                with self.assertRaisesRegex(ValueError, 'updater public key'):
                    self.generate()
                self.assertFalse((self.dist / 'latest.json').exists())

    def test_release_date_must_carry_an_offset(self):
        for pub_date in ['2026-09-24T12:00:00', '2026-09-24', 'yesterday', '']:
            with self.subTest(pub_date=pub_date), self.assertRaisesRegex(ValueError, 'commit date'):
                self.generate(pub_date)
        self.generate('2026-09-24T09:00:00Z')
        self.assertEqual(json.loads((self.dist / 'latest.json').read_text())['pub_date'], '2026-09-24T09:00:00Z')

    def test_unknown_placeholders_fail_instead_of_shipping(self):
        self.write('.github/packaging/homebrew/jet-app.rb.in', 'cask "jet-app" do\n  version "@APP_VERSION@"\nend\n')
        with self.assertRaisesRegex(ValueError, 'APP_VERSION'):
            self.generate()
        self.assertFalse((self.dist / 'jetd.rb').exists())

    def test_invalid_and_mismatched_tags_are_rejected(self):
        for tag in ['main', 'v9999.0.0', 'v1.2.3/../../bad', 'v1.2.3";system("bad")']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release_assets.version_for(tag)

    def test_desktop_app_must_share_the_release_version(self):
        # ADR-0053: one release version for the core and the GUIs.
        for versions in [('0.1.0', None), (self.VERSION, '0.1.0')]:
            with self.subTest(versions=versions):
                self.app_versions(*versions)
                with self.assertRaisesRegex(ValueError, 'desktop app version'):
                    release_assets.version_for(self.tag)

    def test_test_tap_renders_local_urls_for_the_built_labels_only(self):
        dist, desktop, tap = (self.dist.parent / name for name in ('one-dist', 'one-desktop', 'tap'))
        dist.mkdir()
        desktop.mkdir()
        label = 'x86_64-unknown-linux-gnu'
        self.payload(label, dist=dist)
        with tarfile.open(dist / f'jet-core-symbols-{self.VERSION}-{label}.tar.gz', 'w:gz'):
            pass
        self.bundles(label, desktop=desktop, signed=False)
        release_assets.render_test_tap(self.tag, dist, desktop, tap, 'file:///tmp/jet-assets/')
        self.assertEqual(sorted(p.name for p in tap.iterdir()), ['jet-app.rb', 'jetd.rb'])
        formula = (tap / 'jetd.rb').read_text()
        cask = (tap / 'jet-app.rb').read_text()
        payload = dist / f'jet-core-{self.VERSION}-{label}.tar.gz'
        self.assertIn(f'url "file:///tmp/jet-assets/{payload.name}"', formula)
        self.assertIn(hashlib.sha256(payload.read_bytes()).hexdigest(), formula)
        self.assertEqual(formula.count(release_assets.UNBUILT), 2)
        appimage = desktop / f'Jet_{self.VERSION}_amd64.AppImage'
        self.assertIn(f'x86_64_linux: "{hashlib.sha256(appimage.read_bytes()).hexdigest()}"', cask)
        self.assertIn(f'arm64_linux:  "{release_assets.UNBUILT}"', cask)
        self.assertIn('url "file:///tmp/jet-assets/Jet_#{version}_#{arch}.AppImage"', cask)
        for url in ['https://example.com/assets', 'file:///tmp/a"; system("id"); "', 'file:///tmp/../etc',
                    'file://host/x', 'file:////']:
            with self.subTest(url=url), self.assertRaisesRegex(ValueError, 'file:///'):
                release_assets.render_test_tap(self.tag, dist, desktop, tap, url)


class HomebrewTemplates(unittest.TestCase):
    """What the tap must keep true across edits to the templates (spec: formula jetd, cask jet-app)."""

    def template(self, name):
        return (ROOT / '.github/packaging/homebrew' / name).read_text()

    def test_formula_is_jetd_and_guards_an_orphaned_service(self):
        formula = self.template('jetd.rb.in')
        self.assertIn('class Jetd < Formula', formula)
        self.assertIn('name macos: "com.apexgang.jet.homebrew", linux: "jet-homebrew"', formula)
        self.assertIn('ConditionFileIsExecutable=#{opt_bin}/jetd', formula)
        self.assertIn('brew services start apexgang/tap/jetd', formula)
        # `brew audit` rejects a `version` the release URL already carries.
        self.assertNotRegex(formula, r'(?m)^\s*version ')

    def test_cask_is_linux_only_and_depends_on_the_tap_formula(self):
        cask = self.template('jet-app.rb.in')
        self.assertIn('cask "jet-app" do', cask)
        self.assertIn('depends_on formula: "apexgang/tap/jetd"', cask)
        self.assertIn('depends_on :linux', cask)
        self.assertIn('app_image "Jet_#{version}_#{arch}.AppImage", target: "Jet.AppImage"', cask)
        self.assertIn('brew install apexgang/tap/jetd apexgang/tap/jet-app', cask)
        # Homebrew upgrades the app and the daemon together (ADR-0026), and
        # legacy flight blocks are deprecated.
        for absent in ('auto_updates', 'postflight', 'preflight', '.jet"', '/.jet/'):
            self.assertNotIn(absent, cask)

    def test_upgrades_keep_the_launcher_entry_the_app_wrote(self):
        # `brew upgrade` runs the old version's uninstall stanza, which would
        # trash the entry and icon; only zap may remove them.
        cask = self.template('jet-app.rb.in')
        self.assertNotRegex(cask, r'(?m)^\s*uninstall ')
        zap = cask.split('\n  zap trash: [\n', 1)[1].split('\n  ]\n', 1)[0]
        for path in ('/applications/me.heeka.jet-tauri.desktop"',
                     '/icons/hicolor/128x128/apps/me.heeka.jet-tauri.png"'):
            self.assertIn(path, zap)


class ReleaseWorkflows(unittest.TestCase):
    """Shape of the release and packaging workflows that nothing else checks before a tag."""
    WORKFLOWS = ROOT / '.github/workflows'

    def test_every_workflow_is_pinned_through_the_lockfile(self):
        lock = (self.WORKFLOWS / 'actions.lock').read_text()
        for workflow in sorted(self.WORKFLOWS.glob('*.yml')):
            with self.subTest(workflow=workflow.name):
                text = workflow.read_text()
                self.assertTrue(text.startswith('# This workflow is managed by gh actions-lock.\n'))
                self.assertIn(f"'.github/workflows/{workflow.name}':", lock)
                for action in re.findall(r'uses: ([\w.-]+/[\w.-]+)(?:/[\w./-]+)?@(\S+)', text):
                    self.assertIn(f"'{action[0]}@{action[1]}'", lock)

    def test_publication_waits_for_every_build_and_the_homebrew_check(self):
        release = (self.WORKFLOWS / 'release.yml').read_text()
        self.assertIn('needs: [package, desktop-linux, homebrew-check, desktop-e2e]', release)
        self.assertIn('uses: ./.github/workflows/desktop-linux.yml', release)
        self.assertIn('uses: ./.github/workflows/homebrew-check.yml', release)
        # Every tap writer shares one lock, so two pushes never race.
        self.assertEqual(release.count('group: homebrew-tap-jet'), 1)
        self.assertEqual(len(re.findall(r'(?m)^    concurrency:', release)), 1)

    def job(self, workflow, name):
        text = (self.WORKFLOWS / workflow).read_text()
        return re.search(rf'(?ms)^  {name}:\n(.*?)(?=^  [\w-]+:\n|\Z)', text).group(1)

    def test_desktop_journey_drives_the_bundles_before_anything_is_published(self):
        # Wave 4 spec F: the unsigned rehearsal and the signed release both
        # install the bundle desktop-linux built and drive it end to end.
        for workflow, signed in (('packaging.yml', 'false'), ('release.yml', 'true')):
            with self.subTest(workflow=workflow):
                job = self.job(workflow, 'desktop-e2e')
                self.assertIn('needs: desktop-linux', job)
                self.assertIn('uses: ./.github/workflows/desktop-e2e.yml', job)
                self.assertIn(f'signed: {signed}', job)
        journey = (self.WORKFLOWS / 'desktop-e2e.yml').read_text()
        self.assertIn('on:\n  workflow_call:', journey)
        self.assertIn('permissions:\n  contents: read', journey)
        self.assertRegex(journey, r'TAURI_DRIVER_VERSION: \d+\.\d+\.\d+\n')
        self.assertIn('cargo install tauri-driver --version "$TAURI_DRIVER_VERSION" --locked', journey)
        self.assertIn('name: jet-desktop-linux-${{ matrix.label }}', journey)
        self.assertIn('xvfb-run', journey)
        self.assertIn('bun tests/e2e/main.ts --dry-run', journey)
        self.assertIn('sudo loginctl enable-linger', journey)
        # Diagnostics on failure, and the measurements and screenshots always.
        self.assertIn("if: ${{ failure() }}", journey)
        self.assertIn('_SYSTEMD_USER_UNIT=jetd.service', journey)
        upload = journey.split('actions/upload-artifact', 1)[1]
        self.assertIn('if: ${{ !cancelled() }}', upload)
        self.assertIn('path: ${{ runner.temp }}/jet-e2e', upload)
        self.assertEqual(len(re.findall(r'(?m)^    timeout-minutes:', journey)), 1)
        # The journey job has no secret and no write permission.
        self.assertNotIn('secrets', journey)
        self.assertNotIn('write', journey)
        # A pull request that changes the journey, the marks it times or the
        # provisioning it checks runs it before a tag does.
        paths = re.findall(r"(?m)^      - '([^']+)'$", (self.WORKFLOWS / 'packaging.yml').read_text())
        for path in ('.github/workflows/desktop-e2e.yml', 'apps/jet-tauri/tests/e2e/**',
                     'apps/jet-tauri/src/lib/features/shell/timing.ts',
                     'apps/jet-tauri/src/lib/features/system/local-service.svelte.ts',
                     'apps/jet-tauri/src-tauri/src/jet/local_service/**'):
            self.assertIn(path, paths)
            if '*' not in path:
                self.assertTrue((ROOT / path).is_file(), path)

    def test_packaging_rehearsal_runs_for_every_dependency_bump(self):
        # A Tauri CLI bump changes the bundle names release_assets.py expects;
        # it must fail on its pull request, not on the next tag.
        paths = re.findall(r"(?m)^      - '([^']+)'$", (self.WORKFLOWS / 'packaging.yml').read_text())
        dependabot = (ROOT / '.github/dependabot.yml').read_text()
        rewritten = {'cargo': 'Cargo.lock', 'bun': 'bun.lock', 'rust-toolchain': 'rust-toolchain.toml'}
        updates = re.findall(r'package-ecosystem: (\S+)\n    directory: /(\S*)', dependabot)
        self.assertIn(('bun', 'apps/jet-tauri'), updates)
        for ecosystem, directory in updates:
            if ecosystem != 'github-actions':
                with self.subTest(ecosystem=ecosystem, directory=directory):
                    self.assertIn(f'{directory}/{rewritten[ecosystem]}', paths)

    def run_step(self, workflow, name, env):
        """Runs a step's script with the shell GitHub gives it on Linux."""
        text = (self.WORKFLOWS / workflow).read_text()
        lines = text.splitlines()
        start = lines.index(f'      - name: {name}')
        run = lines.index('        run: |', start)
        body = []
        for line in lines[run + 1:]:
            if line.strip() and not line.startswith(' ' * 10):
                break
            body.append(line[10:])
        # Without a `shell`, GitHub runs `bash -e`; naming `bash` in the step or
        # the workflow's defaults adds pipefail.
        self.assertLessEqual(set(re.findall(r'(?m)^ *shell: (.*)$', text)), {'bash'})
        named = any(line.strip() == 'shell: bash' for line in lines[start:run]) or \
            re.search(r'(?m)^defaults:\n  run:\n(?:    .*\n)*?    shell: bash$', text)
        shell = ['bash', '--noprofile', '--norc', '-eo', 'pipefail'] if named else ['bash', '-e']
        return subprocess.run([*shell, '-c', '\n'.join(body)], env=env, capture_output=True, text=True)

    def test_homebrew_install_trusts_the_tap_only_after_a_trust_refusal(self):
        documented = 'install apexgang/tap/jetd apexgang/tap/jet-app'
        for refusal in ('', 'Error: Refusing to load cask jet-app from untrusted tap apexgang/tap.',
                        'Error: Download failed'):
            with self.subTest(refusal=refusal), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'bin').mkdir()
                (root / 'home').mkdir()
                brew = root / 'bin/brew'
                brew.write_text('''#!/usr/bin/env bash
echo "$*" >> "$TEST_CALLS"
case "$1" in
  install)
    if [[ -n $TEST_REFUSAL && ! -e $TEST_CALLS.trusted ]]; then echo "$TEST_REFUSAL" >&2; exit 1; fi
    mkdir -p "$HOME/Applications"
    : > "$HOME/Applications/Jet.AppImage"
    chmod +x "$HOME/Applications/Jet.AppImage" ;;
  trust) [[ $2 == apexgang/tap ]] && : > "$TEST_CALLS.trusted" ;;
  test) [[ -x $HOME/Applications/Jet.AppImage ]] || { echo 'Error: No such keg' >&2; exit 1; } ;;
esac
''')
                brew.chmod(0o755)
                calls = root / 'calls'
                env = {'PATH': f'{root / "bin"}:/usr/bin:/bin', 'HOME': str(root / 'home'),
                       'RUNNER_TEMP': str(root), 'TEST_CALLS': str(calls), 'TEST_REFUSAL': refusal}
                result = self.run_step('homebrew-check.yml', 'Install with the documented command', env)
                history = calls.read_text().splitlines()
                if not refusal:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(history[0], documented)
                    self.assertNotIn('trust apexgang/tap', history)
                elif 'untrusted tap' in refusal:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(history[:3], [documented, 'trust apexgang/tap', documented])
                    self.assertIn('::warning::', result.stdout)
                else:
                    # Any other failure fails the step before brew is asked again.
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(history, [documented])


if __name__ == '__main__':
    unittest.main()
