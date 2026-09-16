"""Validate a tagged release and generate checksums and its binary Homebrew formula."""
import argparse
import hashlib
import json
import re
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LABELS = {
    'universal-apple-darwin': 'MACOS',
    'aarch64-unknown-linux-gnu': 'LINUX_ARM',
    'x86_64-unknown-linux-gnu': 'LINUX_INTEL',
}


def version_for(tag):
    # ASVS 1.2.5: only version syntax may reach URLs, Ruby source and Git arguments.
    if not re.fullmatch(r'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?', tag):
        raise ValueError(f'expected a vMAJOR.MINOR.PATCH tag, got {tag!r}')
    version = tag[1:]
    workspace = tomllib.loads((ROOT / 'packages/Cargo.toml').read_text())
    if version != workspace['workspace']['package']['version']:
        raise ValueError('release tag must equal the workspace version')
    return version


def generate(tag, dist):
    version = version_for(tag)
    replacements = {'VERSION': version, 'URL': f'https://github.com/apexgang/jet/releases/download/{tag}'}
    checksums = {}
    expected = {f'jet-core{symbols}-{version}-{label}.tar.gz'
                for label in LABELS for symbols in ('', '-symbols')}
    if {p.name for p in dist.glob('*.tar.gz')} != expected:
        raise ValueError('release requires exactly one payload and symbol archive for every target')
    for name in sorted(expected):
        with (dist / name).open('rb') as file:
            checksums[name] = hashlib.file_digest(file, 'sha256').hexdigest()
    for label, key in LABELS.items():
        name = f'jet-core-{version}-{label}'
        with tarfile.open(dist / f'{name}.tar.gz') as archive:
            # Read the named manifest without extracting archive paths.
            member = archive.getmember(f'{name}/manifest.json')
            if not member.isfile() or member.size > 65536:
                raise ValueError('invalid release manifest')
            manifest = json.load(archive.extractfile(member))
        if (manifest['version'], manifest['target']) != (version, label):
            raise ValueError(f'{name}: manifest does not match the release')
        replacements[f'{key}_SHA256'] = checksums[f'{name}.tar.gz']
    template = (ROOT / '.github/packaging/homebrew/jet.rb.in').read_text()
    formula = re.sub(r'@([A-Z_0-9]+)@', lambda match: replacements[match[1]], template)
    (dist / 'jet.rb').write_text(formula)
    (dist / 'SHA256SUMS').write_text(''.join(f'{digest}  {name}\n' for name, digest in checksums.items()))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--dist', type=Path)
    args = parser.parse_args()
    if args.dist is None:
        print(version_for(args.tag))
    else:
        generate(args.tag, args.dist)
