import os, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


def run(env, *args):
    return subprocess.run([sys.executable, f'{HERE}/freeze.py', *args], env=env, capture_output=True, text=True)


def test_add_check_and_refuse():
    with tempfile.TemporaryDirectory() as d:
        env = {**os.environ, 'OBOETE_EVAL': d}
        q = os.path.join(d, 'q.jsonl')
        with open(q, 'w') as f:
            f.write('{"qid":"p1"}\n')
        assert run(env, 'add', 'q.jsonl').returncode == 0
        assert run(env, 'check').returncode == 0
        # A frozen file is never re-frozen: a changed input is a new set (spec 8.1).
        assert run(env, 'add', 'q.jsonl').returncode != 0
        with open(q, 'a') as f:
            f.write('{"qid":"p2"}\n')
        r = run(env, 'check')
        assert r.returncode == 1 and 'changed: q.jsonl' in r.stdout
        os.remove(q)
        assert 'missing: q.jsonl' in run(env, 'check').stdout
        assert os.stat(os.path.join(d, 'freeze.json')).st_mode & 0o777 == 0o600
