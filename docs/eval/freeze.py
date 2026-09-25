"""Milestone 1: freeze the evaluation inputs (docs/spec.md 8.4 item 1; docs/milestone-1.md).

  freeze.py add <path under the eval dir>...   record sha256 and size in freeze.json
  freeze.py check                              exit 1 if a frozen file changed or went missing
A frozen file is never re-added: a changed input is a new set, not an update (spec 8.1)."""
import datetime, json, os, sys

from common import E, owner_only, sha256_file

MANIFEST = os.path.join(E, 'freeze.json')


def load():
    if not os.path.exists(MANIFEST):
        return {'files': {}}
    with open(MANIFEST) as f:
        return json.load(f)


def add(paths):
    m = load()
    for rel in paths:
        if rel in m['files']:
            sys.exit(f'{rel} is already frozen; a changed input is a new set (docs/spec.md 8.1)')
        full = os.path.join(E, rel)
        m['files'][rel] = {
            'sha256': sha256_file(full),
            'bytes': os.path.getsize(full),
            'frozen_at': datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds'),
        }
    with open(MANIFEST, 'w') as f:
        json.dump(m, f, indent=1, sort_keys=True)


def check():
    bad = []
    for rel, want in sorted(load()['files'].items()):
        full = os.path.join(E, rel)
        if not os.path.exists(full):
            bad.append(f'missing: {rel}')
        elif sha256_file(full) != want['sha256']:
            bad.append(f'changed: {rel}')
    manifest = os.path.join(E, 'replay', 'manifest.json')
    if 'replay/manifest.json' in load()['files'] and os.path.exists(manifest):
        with open(manifest) as f:
            for s in json.load(f)['sessions']:
                for rel, digest in s['files'].items():
                    full = os.path.join(E, 'replay', rel)
                    if not os.path.exists(full):
                        bad.append(f'missing: replay/{rel}')
                    elif sha256_file(full) != digest:
                        bad.append(f'changed: replay/{rel}')
    return bad


if __name__ == '__main__':
    owner_only()
    if sys.argv[1:2] == ['add'] and len(sys.argv) > 2:
        add(sys.argv[2:])
    elif sys.argv[1:] == ['check']:
        bad = check()
        print('\n'.join(bad) or f'ok: {len(load()["files"])} frozen files unchanged')
        sys.exit(1 if bad else 0)
    else:
        sys.exit(__doc__)
