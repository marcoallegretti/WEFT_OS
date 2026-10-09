"""Check that weft-runtime enforces the grants weft-appd passes it.

The harness runs the real runtime (built with wasmtime-runtime and
net-fetch) on the test component in tests/components/grants-probe, which
calls every grant-controlled operation once and prints its outcome. Each
scenario passes a different set of `--grant` and `--preopen` arguments:

- no grants: no /data, and every host import reports that its capability
  is not granted;
- read-only data: /data can be read but not written, and nothing appears
  on the host;
- read-write data: the write reaches the host directory;
- notifications and clipboard read: those imports are attempted (they may
  still fail for lack of a desktop service) while clipboard write stays
  denied;
- a host-specific fetch grant: requests to that host return their real
  status (200, 404, and 302 without following the redirect to another
  host), and requests to another host are denied;
- a wildcard fetch grant: the other host is reachable.

It also checks that the runtime refuses filesystem capabilities passed as
`--grant` and preopens without an access mode.

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
        self.store = work / "store"
        package = self.store / APP_ID
        package.mkdir(parents=True)
        (package / "wapp.toml").write_text(
            f'[package]\nid = "{APP_ID}"\nname = "Grants probe"\nversion = "0.1.0"\n'
            '[runtime]\nmodule = "app.wasm"\n[ui]\nentry = "ui/index.html"\n')
        self.package = package
        self.targets = work / "targets"
        self.targets.mkdir()
        (self.targets / "targets.txt").write_text(
            f"fetch-ok http://127.0.0.1:{port}/ok\n"
            f"fetch-missing http://127.0.0.1:{port}/missing\n"
            f"fetch-redirect http://127.0.0.1:{port}/redirect\n"
            f"fetch-other-host http://localhost:{port}/ok\n")

    def run(self, *arguments, expect_success=True):
        command = [str(self.runtime), APP_ID, "1",
                   "--preopen", f"{self.targets}::/probe::ro", *arguments]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60,
                                env={"WEFT_APP_STORE": str(self.store), "PATH": "/usr/bin:/bin",
                                     "RUST_LOG": "warn"})
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
    expect(probes, "data-write", False)
    if (data / "written.txt").exists():
        raise AssertionError("read-only grant allowed a write on the host")

    _, probes = runner.run("--preopen", f"{data}::/data::rw")
    expect(probes, "data-write", True)
    if (data / "written.txt").read_text() != "probe":
        raise AssertionError("read-write grant did not write to the host directory")

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

    _, probes = runner.run("--grant", "net:fetch:*")
    expect(probes, "fetch-other-host", True, contains="200")

    for arguments in (["--grant", "fs:rw:app-data"], ["--preopen", f"{data}::/data"],
                      ["--grant", "sys:everything"]):
        result, _ = runner.run(*arguments, expect_success=False)
        if result.returncode == 0:
            raise AssertionError(f"runtime accepted {' '.join(arguments)}")


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
        shutil.copy(probe, runner.package / "app.wasm")
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
