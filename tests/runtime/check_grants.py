"""Check that weft-runtime enforces the grants weft-appd passes it.

The harness runs the real runtime (built with wasmtime-runtime and
net-fetch) on the test component in tests/components/grants-probe, which
calls every grant-controlled operation once and prints its outcome. Each
scenario passes a different set of `--grant` and `--preopen` arguments:

- no grants: no /data, and every host import reports that its capability
  is not granted;
- read-only data: /data can be read, but creating, overwriting, renaming
  and removing files and creating directories all fail and the host
  directory is unchanged;
- read-write data: the write reaches the host directory;
- notifications and clipboard read: those imports are attempted (they may
  still fail for lack of a desktop service) while clipboard write stays
  denied;
- a host-specific fetch grant: requests to that host return their real
  status (200, 404, and 302 without following the redirect to another
  host); requests to another host, a forged `Host` header, a method that
  injects a request line and a `file:` URL are refused;
- a wildcard fetch grant: the other host is reachable;
- limits: an allocation beyond the default memory limit (256 MiB) fails
  inside the component, and succeeds with `--max-memory-mib 512`; app
  messages longer than 64 KiB or containing a line break are refused.

It also checks that the runtime refuses, with a specific error, filesystem
capabilities passed as `--grant`, unknown capabilities and preopens
without an access mode.

Requires cargo with the wasm32-wasip2 target.
"""

import argparse
import http.server
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROBE_DIR = ROOT / "tests/components/grants-probe"
APP_ID = "org.weft.test.grants"
NOT_GRANTED = "is not granted"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/ok":
            self.send_response(200)
        elif self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_port}/ok")
        else:
            self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


def build_probe():
    subprocess.run(["cargo", "build", "-q", "--release", "--target", "wasm32-wasip2"],
                   cwd=PROBE_DIR, check=True)
    return PROBE_DIR / "target/wasm32-wasip2/release/app.wasm"


class Runner:
    def __init__(self, runtime, work, port):
        self.runtime = runtime
        self.work = work
        self.module = work / "app.wasm"
        self.targets = work / "targets"
        self.targets.mkdir()
        (self.targets / "targets.txt").write_text(
            f"fetch-ok http://127.0.0.1:{port}/ok\n"
            f"fetch-missing http://127.0.0.1:{port}/missing\n"
            f"fetch-redirect http://127.0.0.1:{port}/redirect\n"
            f"fetch-other-host http://localhost:{port}/ok\n")

    def run(self, *arguments, expect_success=True):
        command = [str(self.runtime), APP_ID, "1", "--module", str(self.module),
                   "--preopen", f"{self.targets}::/probe::ro", *arguments]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60,
                                env={"PATH": "/usr/bin:/bin", "RUST_LOG": "warn"})
        if expect_success and result.returncode != 0:
            raise AssertionError(f"runtime failed: {' '.join(arguments)}\n{result.stderr}")
        probes = {}
        for line in result.stdout.splitlines():
            if line.startswith("PROBE "):
                name, outcome = line[6:].split(" ", 1)
                probes[name] = outcome
        return result, probes


def expect(probes, name, ok, contains=None, absent=None):
    outcome = probes.get(name)
    if outcome is None:
        raise AssertionError(f"{name}: no result")
    if outcome.startswith("ok") != ok:
        raise AssertionError(f"{name}: expected {'success' if ok else 'failure'}, got {outcome!r}")
    if contains is not None and contains not in outcome:
        raise AssertionError(f"{name}: expected {contains!r} in {outcome!r}")
    if absent is not None and absent in outcome:
        raise AssertionError(f"{name}: did not expect {absent!r} in {outcome!r}")


def check(runner, data):
    _, probes = runner.run()
    expect(probes, "data-read", False)
    expect(probes, "data-write", False)
    for name in ("notifications", "clipboard-read", "clipboard-write",
                 "fetch-ok", "fetch-other-host"):
        expect(probes, name, False, contains=NOT_GRANTED)

    (data / "seed.txt").write_text("seed")
    _, probes = runner.run("--preopen", f"{data}::/data::ro")
    expect(probes, "data-read", True, contains="seed")
    for name in ("data-write", "data-overwrite", "data-mkdir", "data-rename", "data-remove"):
        expect(probes, name, False)
    if sorted(p.name for p in data.iterdir()) != ["seed.txt"] or \
            (data / "seed.txt").read_text() != "seed":
        raise AssertionError("read-only grant allowed the host directory to change")

    _, probes = runner.run("--preopen", f"{data}::/data::rw")
    for name in ("data-write", "data-overwrite", "data-mkdir", "data-rename"):
        expect(probes, name, True)
    if (data / "written.txt").read_text() != "probe" or \
            (data / "moved.txt").read_text() != "changed" or not (data / "made").is_dir():
        raise AssertionError("read-write grant did not change the host directory")

    _, probes = runner.run("--grant", "sys:notifications", "--grant", "sys:clipboard:read")
    expect(probes, "notifications", probes.get("notifications", "").startswith("ok"),
           absent=NOT_GRANTED)
    expect(probes, "clipboard-read", probes.get("clipboard-read", "").startswith("ok"),
           absent=NOT_GRANTED)
    expect(probes, "clipboard-write", False, contains=NOT_GRANTED)

    _, probes = runner.run("--grant", "net:fetch:127.0.0.1")
    expect(probes, "fetch-ok", True, contains="200")
    expect(probes, "fetch-missing", True, contains="404")
    expect(probes, "fetch-redirect", True, contains="302")
    expect(probes, "fetch-other-host", False, contains=NOT_GRANTED)
    expect(probes, "fetch-host-header", False, contains="set by the runtime")
    expect(probes, "fetch-bad-method", False, contains="unsupported HTTP method")
    expect(probes, "fetch-file-scheme", False, contains="unsupported URL scheme")

    _, probes = runner.run("--grant", "net:fetch:*")
    expect(probes, "fetch-other-host", True, contains="200")

    _, probes = runner.run()
    expect(probes, "memory-grow", False)
    expect(probes, "ipc-newline", False, contains="line break")
    expect(probes, "ipc-oversize", False, contains="exceeds")
    _, probes = runner.run("--max-memory-mib", "512")
    expect(probes, "memory-grow", True)

    for arguments, message in (
            (["--grant", "fs:rw:app-data"], "is not granted through --grant"),
            (["--preopen", f"{data}::/data"], "--preopen expects HOST::GUEST::ro|rw"),
            (["--preopen", f"{data}::/data::wx"], "--preopen mode must be ro or rw"),
            (["--grant", "sys:everything"], "unknown capability 'sys:everything'"),
            (["--max-memory-mib", "0"], "invalid --max-memory-mib")):
        result, _ = runner.run(*arguments, expect_success=False)
        if result.returncode == 0 or message not in result.stderr:
            raise AssertionError(f"runtime did not refuse {' '.join(arguments)} with "
                                 f"{message!r}:\n{result.stderr}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runtime", type=Path, default=ROOT / "target/debug/weft-runtime",
                        help="weft-runtime built with wasmtime-runtime and net-fetch")
    parser.add_argument("--probe", type=Path, help="prebuilt probe component")
    args = parser.parse_args(argv)
    if not args.runtime.is_file():
        print(f"missing: {args.runtime}", file=sys.stderr)
        return 2

    probe = args.probe or build_probe()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    work = Path(tempfile.mkdtemp(prefix="weft-grants-"))
    try:
        runner = Runner(args.runtime, work, server.server_port)
        shutil.copy(probe, runner.module)
        data = work / "data"
        data.mkdir()
        check(runner, data)
    except AssertionError as failure:
        print(f"grants check failed: {failure}", file=sys.stderr)
        return 1
    finally:
        server.shutdown()
        shutil.rmtree(work, ignore_errors=True)
    print("grants check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
