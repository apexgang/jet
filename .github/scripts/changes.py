"""Select CI jobs from a complete Git diff; unknown history runs every gate."""
import os
import subprocess


def classify(paths):
    automation = any(p.startswith('.github/') and p != '.github/workflows/README.md'
                     and not p.startswith('.github/docs/') for p in paths)
    shared = automation or any(p in {
        'packages/Cargo.toml', 'packages/Cargo.lock', 'packages/justfile',
        'packages/rust-toolchain.toml', 'packages/.cargo/config.toml',
    } for p in paths)
    core = shared or any(p.startswith('packages/') or p == 'docs/conformance-matrix.md'
                         for p in paths)
    contracts = shared or any(p.startswith((
        'packages/jet-protocol/', 'apps/jet/jet/Protocol/',
        'apps/jet-tauri/src/lib/protocol/',
    )) for p in paths)
    return {'core': core, 'contracts': contracts}


def main():
    base = os.environ.get('BASE_SHA', '')
    try:
        if not base or set(base) == {'0'}:
            raise ValueError('no base commit')
        # ASVS 1.2.5: Git refs are separate arguments, never shell source.
        subprocess.run(['git', 'fetch', '--no-tags', '--depth=1', 'origin', base], check=True)
        result = subprocess.run(['git', 'diff', '--name-only', '--no-renames', '-z', base, 'HEAD', '--'],
                                check=True, capture_output=True)
        jobs = classify(result.stdout.decode().split('\0'))
    except (ValueError, UnicodeError, subprocess.CalledProcessError):
        jobs = {'core': True, 'contracts': True}
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        for job, enabled in jobs.items():
            print(f'{job}={str(enabled).lower()}', file=output)


if __name__ == '__main__':
    main()
