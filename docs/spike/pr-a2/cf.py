"""Tiny Cloudflare API helper for the PR-A2 spike. Reads ~/CF_API.md (email, global key); never prints the key."""
import json, os, urllib.request
_email, _key = open(os.path.expanduser('~/CF_API.md')).read().split()[:2]
def need(ok, what):
    """Stop the run on a failed call (not `assert`, which `python -O` removes)."""
    if not ok:
        raise SystemExit(f'failed: {what}')
def call(method, path, body=None, timeout=60):
    req = urllib.request.Request('https://api.cloudflare.com/client/v4' + path, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={'X-Auth-Email': _email, 'X-Auth-Key': _key, 'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b'{}')
