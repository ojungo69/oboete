"""Offline evaluation tests never open the owner's store or call a live provider."""
import os, shutil, socket, subprocess, sys, tempfile

import pytest

_run, _popen, _connect = subprocess.run, subprocess.Popen, socket.socket.connect
_freeze = os.path.join(os.path.dirname(__file__), 'freeze.py')

_eval = tempfile.mkdtemp(prefix='oboete-eval-tests-')
os.environ['OBOETE_EVAL'] = _eval
sys.path.insert(0, os.path.dirname(__file__))


def pytest_sessionfinish(session, exitstatus):
    shutil.rmtree(_eval)


@pytest.fixture(autouse=True)
def offline(monkeypatch, tmp_path, tmp_path_factory):
    import common

    for key in list(os.environ):
        if any(s in key.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD')):
            monkeypatch.delenv(key)

    def refused(*args, **kwargs):
        raise AssertionError('evaluation tests require an injected offline stub')

    base = os.fspath(tmp_path_factory.getbasetemp()) + os.sep

    def local(tool, argv, **kwargs):
        # The children the other tests start: freeze.py's pure file checker, a fake binary a test
        # wrote under pytest's temporary directory (m4.py's replay), and git asked for a commit's
        # time (m4.py's gate).
        program = os.fspath(argv[0]) if argv else ''
        if not (argv[:2] == [sys.executable, _freeze] or program.startswith(base)
                or list(argv[:3]) == ['git', 'show', '-s']):
            return refused()
        kwargs['env'] = {k: v for k, v in kwargs.get('env', common.clean_env()).items()
                         if not any(s in k.upper() for s in ('TOKEN', 'KEY', 'SECRET', 'PASSWORD'))}
        return tool(argv, **kwargs)

    def loopback(sock, address):
        # A test's own server on this machine (test_label.py) is not a live provider.
        host = address[0] if isinstance(address, tuple) else None
        if host in ('127.0.0.1', '::1', 'localhost'):
            return _connect(sock, address)
        return refused()

    monkeypatch.setattr(common, 'E', str(tmp_path / 'eval'))
    monkeypatch.setattr(subprocess, 'run', lambda argv, **kwargs: local(_run, argv, **kwargs))
    monkeypatch.setattr(subprocess, 'Popen', lambda argv, **kwargs: local(_popen, argv, **kwargs))
    # urlopen and create_connection both end in connect.
    monkeypatch.setattr(socket.socket, 'connect', loopback)
