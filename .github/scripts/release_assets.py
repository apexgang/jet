"""Validate a tagged release and generate its checksums, Homebrew formula and cask, and updater manifest."""
import argparse
import base64
import binascii
import datetime
import hashlib
import json
import re
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RELEASES = 'https://github.com/apexgang/jet/releases/download'
# Core payload labels and the template key their payload checksum fills.
LABELS = {
    'universal-apple-darwin': 'MACOS',
    'aarch64-unknown-linux-gnu': 'LINUX_ARM',
    'x86_64-unknown-linux-gnu': 'LINUX_INTEL',
}
# Tauri's Linux bundle names per core label, keyed by the updater's bundle
# type, with the architecture the updater's `linux-<arch>-<type>` keys use.
DESKTOP = {
    'aarch64-unknown-linux-gnu': ('aarch64', {
        'appimage': 'Jet_{version}_aarch64.AppImage',
        'deb': 'Jet_{version}_arm64.deb',
        'rpm': 'Jet-{version}-1.aarch64.rpm',
    }),
    'x86_64-unknown-linux-gnu': ('x86_64', {
        'appimage': 'Jet_{version}_amd64.AppImage',
        'deb': 'Jet_{version}_amd64.deb',
        'rpm': 'Jet-{version}-1.x86_64.rpm',
    }),
}
APP = 'apps/jet-tauri'
TEMPLATES = '.github/packaging/homebrew'
# The tagged release renders these. `jet.rb.in` beside them is the macOS core
# formula of the Swift app's release (`apexgang/tap/jet`), which renders it
# itself.
FORMULAE = ('jetd.rb.in', 'jet-app.rb.in')
# A test tap renders labels it has no payload for with this checksum; nothing downloads them.
UNBUILT = '0' * 64


def version_for(tag):
    # ASVS 1.2.5: only version syntax may reach URLs, Ruby source and Git arguments.
    if not re.fullmatch(r'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?', tag):
        raise ValueError(f'expected a vMAJOR.MINOR.PATCH tag, got {tag!r}')
    version = tag[1:]
    workspace = tomllib.loads((ROOT / 'packages/Cargo.toml').read_text())
    if version != workspace['workspace']['package']['version']:
        raise ValueError('release tag must equal the workspace version')
    # ADR-0053: the desktop app ships under the same release version as the core.
    app = {
        'tauri.conf.json': json.loads((ROOT / APP / 'src-tauri/tauri.conf.json').read_text()).get('version'),
        'Cargo.toml': tomllib.loads((ROOT / APP / 'src-tauri/Cargo.toml').read_text())['package'].get('version'),
        'package.json': json.loads((ROOT / APP / 'package.json').read_text()).get('version'),
    }
    for source, found in app.items():
        if found != version:
            raise ValueError(f'the desktop app version in {source} ({found}) '
                             f'must equal the workspace version {version}')
    return version


def payloads(dist, version, labels):
    """Return every core archive's SHA-256 after checking the set and each manifest."""
    expected = {f'jet-core{symbols}-{version}-{label}.tar.gz'
                for label in labels for symbols in ('', '-symbols')}
    if {p.name for p in dist.glob('*.tar.gz')} != expected:
        raise ValueError('release requires exactly one payload and symbol archive for every target')
    for label in labels:
        name = f'jet-core-{version}-{label}'
        with tarfile.open(dist / f'{name}.tar.gz') as archive:
            # Read the named manifest without extracting archive paths.
            member = archive.getmember(f'{name}/manifest.json')
            if not member.isfile() or member.size > 65536:
                raise ValueError('invalid release manifest')
            manifest = json.load(archive.extractfile(member))
        if (manifest['version'], manifest['target']) != (version, label):
            raise ValueError(f'{name}: manifest does not match the release')
    return {name: digest(dist / name) for name in sorted(expected)}


def bundles(desktop, version, labels, signed):
    """Return every desktop bundle name by label and type after checking the set."""
    names = {label: {kind: pattern.format(version=version) for kind, pattern in DESKTOP[label][1].items()}
             for label in labels}
    expected = {f'{name}{suffix}' for kinds in names.values() for name in kinds.values()
                for suffix in (('', '.sig') if signed else ('',))}
    if {p.name for p in desktop.iterdir()} != expected:
        raise ValueError('release requires exactly the deb, rpm and AppImage bundles'
                         f'{" and their signatures" if signed else ""} for every Linux target')
    for name in expected:
        if not (desktop / name).is_file() or (desktop / name).is_symlink():
            raise ValueError(f'{name}: not a regular file')
    return names


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def minisign(line, size):
    """Algorithm, key id and the rest of a base64 minisign public key (42 bytes) or signature (74) line."""
    raw = base64.b64decode(line, validate=True)
    if len(raw) != size or raw[:2] not in (b'Ed', b'ED'):
        raise ValueError('not a minisign Ed25519 key or signature')
    return raw[:2], raw[2:10], raw[10:]


def updater_key():
    """The key id and Ed25519 public key of the updater key the release build embeds."""
    path = ROOT / APP / 'src-tauri/tauri.release.conf.json'
    try:
        config = json.loads(path.read_text())
        public_key = base64.b64decode(config['plugins']['updater']['pubkey'], validate=True).decode()
        algorithm, key_id, key = minisign(public_key.splitlines()[1], 42)
        if algorithm != b'Ed':
            raise ValueError
        return key_id, key
    except (OSError, LookupError, TypeError, ValueError, binascii.Error, UnicodeDecodeError):
        raise ValueError(f'{path.relative_to(ROOT)} must pin the updater public key') from None


# The DER prefix of an Ed25519 SubjectPublicKeyInfo (RFC 8410).
ED25519_PUBLIC_KEY = bytes.fromhex('302a300506032b6570032100')


def ed25519_verifies(key, message, signature):
    """Whether `signature` is the Ed25519 signature of `message` (bytes, or a file) by `key`.

    The standard library has no Ed25519, so OpenSSL (3.0 or later) checks it.
    """
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        pem = base64.encodebytes(ED25519_PUBLIC_KEY + key).decode()
        (scratch / 'key.pem').write_text(f'-----BEGIN PUBLIC KEY-----\n{pem}-----END PUBLIC KEY-----\n')
        (scratch / 'signature').write_bytes(signature)
        if isinstance(message, bytes):
            (scratch / 'message').write_bytes(message)
            message = scratch / 'message'
        try:
            result = subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', scratch / 'key.pem',
                                     '-rawin', '-in', message, '-sigfile', scratch / 'signature'],
                                    capture_output=True, text=True)
        except FileNotFoundError:
            raise RuntimeError('checking the updater signatures needs the openssl command') from None
    if result.returncode == 0 and 'Signature Verified Successfully' in result.stdout:
        return True
    if 'Signature Verification Failure' in result.stdout:
        return False
    raise RuntimeError(f'openssl could not check an Ed25519 signature: {result.stderr.strip()}')


def signature(path, bundle, version, updater):
    """Return a `.sig` file's content: the base64 minisign box Tauri wrote for the `bundle` file.

    The updater takes this content itself, never a path, and validates the whole
    manifest before it reads the version, so one bad entry breaks every update.
    """
    # ASVS 2.2.1: accept only the box Tauri writes for this bundle and version,
    # signed with the key the release configuration pins. Tauri only warns when
    # the signing key does not match that public key, and the artifacts reach
    # this job from other runners, so the signatures are verified here too.
    expected_key, public_key = updater
    content = path.read_bytes()
    try:
        if len(content) > 4096:
            raise ValueError
        box = base64.b64decode(content, validate=True).decode('utf-8').splitlines()
        if (len(box) != 4 or not box[0].startswith('untrusted comment: ')
                or not box[2].startswith('trusted comment: ')):
            raise ValueError
        algorithm, signed_key, signed = minisign(box[1], 74)
        comment = box[2].removeprefix('trusted comment: ')
        global_signature = base64.b64decode(box[3], validate=True)
        if len(global_signature) != 64:
            raise ValueError
    except (ValueError, binascii.Error, UnicodeDecodeError):
        raise ValueError(f'{path.name}: not a Tauri updater signature') from None
    if signed_key != expected_key:
        raise ValueError(f'{path.name}: signed with key {signed_key[::-1].hex().upper()}, '
                         f'not the updater key {expected_key[::-1].hex().upper()}')
    # `ED` signs the BLAKE2b-512 digest of the file, legacy `Ed` the file itself.
    if algorithm == b'ED':
        with bundle.open('rb') as file:
            message = hashlib.file_digest(file, 'blake2b').digest()
    else:
        message = bundle
    if not ed25519_verifies(public_key, message, signed):
        raise ValueError(f'{path.name}: the signature does not verify for the content of {bundle.name}')
    if not ed25519_verifies(public_key, signed + comment.encode(), global_signature):
        raise ValueError(f'{path.name}: the global signature does not cover the trusted comment')
    fields = dict(field.partition(':')[::2] for field in comment.split('\t'))
    # The app sets `requireSignedVersion`, so a signature without the version
    # (`tauri signer sign` without `--app-version`) would fail every update.
    if fields.get('file') != bundle.name or fields.get('version') != version:
        raise ValueError(f'{path.name}: signs {fields.get("file")!r} version {fields.get("version")!r}, '
                         f'not {bundle.name} version {version!r}')
    return content.decode('ascii')


def release_date(value):
    # ASVS 2.2.1: an RFC 3339 instant with an offset, published in UTC.
    try:
        instant = datetime.datetime.fromisoformat(value)
    except ValueError:
        instant = None
    if instant is None or instant.tzinfo is None:
        raise ValueError(f'expected an ISO 8601 commit date with an offset, got {value!r}')
    return instant.astimezone(datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ')


def render(values):
    """Render the release's Homebrew templates: `jetd.rb.in` to `jetd.rb`, `jet-app.rb.in` to `jet-app.rb`."""
    rendered = {}
    for name in FORMULAE:
        template = ROOT / TEMPLATES / name
        text = template.read_text()
        unknown = set(re.findall(r'@([A-Z_0-9]+)@', text)) - set(values)
        if unknown:
            raise ValueError(f'{template.name}: unknown placeholders {sorted(unknown)}')
        rendered[template.name.removesuffix('.in')] = re.sub(r'@([A-Z_0-9]+)@', lambda match: values[match[1]], text)
    return rendered


def replacements(version, url, core, desktop):
    values = {'VERSION': version, 'URL': url}
    for label, key in LABELS.items():
        values[f'{key}_SHA256'] = core.get(f'jet-core-{version}-{label}.tar.gz', UNBUILT)
        if label in DESKTOP:
            values[f'{key}_APPIMAGE_SHA256'] = desktop.get(label, UNBUILT)
    return values


def generate(tag, dist, desktop, pub_date):
    """Write the release's formula, cask, updater manifest, and SHA256SUMS into `dist`."""
    version = version_for(tag)
    published = release_date(pub_date)
    url = f'{RELEASES}/{tag}'
    checksums = payloads(dist, version, LABELS)
    names = bundles(desktop, version, list(DESKTOP), signed=True)
    for kinds in names.values():
        checksums.update((name, digest(desktop / name)) for name in kinds.values())
        checksums.update((f'{name}.sig', digest(desktop / f'{name}.sig')) for name in kinds.values())
    appimages = {label: checksums[kinds['appimage']] for label, kinds in names.items()}
    generated = render(replacements(version, url, checksums, appimages))
    updater = updater_key()
    manifest = {'version': version, 'pub_date': published, 'platforms': {
        f'linux-{DESKTOP[label][0]}-{kind}': {
            'signature': signature(desktop / f'{name}.sig', desktop / name, version, updater),
            'url': f'{url}/{name}',
        }
        for label, kinds in names.items() for kind, name in kinds.items()
    }}
    generated['latest.json'] = json.dumps(manifest, indent=2, sort_keys=True) + '\n'
    for name, text in generated.items():
        (dist / name).write_text(text)
        checksums[name] = digest(dist / name)
    (dist / 'SHA256SUMS').write_text(''.join(f'{checksums[name]}  {name}\n' for name in sorted(checksums)))


def render_test_tap(tag, dist, desktop, out, url):
    """Render the formula and cask against local files for `homebrew-check.yml`.

    Only the labels built here get real checksums; the rest get a placeholder
    that nothing downloads on the test runner.
    """
    version = version_for(tag)
    # ASVS 2.2.1: the URL lands in Ruby source, so accept only a plain absolute file path.
    if not re.fullmatch(r'file://(/[A-Za-z0-9._-]+)+/?', url) or '/../' in f'{url}/':
        raise ValueError(f'expected a file:/// URL of an absolute directory, got {url!r}')
    labels = [label for label in LABELS if (dist / f'jet-core-{version}-{label}.tar.gz').exists()]
    if not labels:
        raise ValueError('no core payload to render')
    core = payloads(dist, version, labels)
    signed = any(desktop.glob('*.sig'))
    names = bundles(desktop, version, [label for label in labels if label in DESKTOP], signed)
    appimages = {label: digest(desktop / kinds['appimage']) for label, kinds in names.items()}
    out.mkdir(parents=True, exist_ok=True)
    for name, text in render(replacements(version, url.rstrip('/'), core, appimages)).items():
        (out / name).write_text(text)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--dist', type=Path, help='directory holding the core payload and symbol archives')
    parser.add_argument('--desktop', type=Path, help='directory holding the Linux desktop bundles')
    parser.add_argument('--pub-date', help="the tagged commit's ISO 8601 date, for the updater manifest")
    parser.add_argument('--test-tap', type=Path, help='render only the formula and cask into this directory')
    parser.add_argument('--url', help='with --test-tap: the file:/// directory serving the archives')
    args = parser.parse_args()
    if args.dist is None:
        if args.desktop or args.pub_date or args.test_tap or args.url:
            parser.error('--desktop, --pub-date, --test-tap and --url need --dist')
        version = version_for(args.tag)
        # Fail before any build when the release could not publish signatures.
        updater_key()
        print(version)
    elif args.test_tap is not None:
        if args.desktop is None or args.url is None or args.pub_date:
            parser.error('--test-tap needs --desktop and --url, and takes no --pub-date')
        render_test_tap(args.tag, args.dist, args.desktop, args.test_tap, args.url)
    else:
        if args.desktop is None or args.pub_date is None or args.url:
            parser.error('a release needs --desktop and --pub-date, and takes no --url')
        generate(args.tag, args.dist, args.desktop, args.pub_date)
