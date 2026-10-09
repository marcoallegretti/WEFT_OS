"""Check that the Notes demo stores exactly what the user types.

The harness runs weft-appd on a nested desktop with the real runtime and app
shell, seeds the Notes data with text containing a literal backslash
sequence, quotes, a tab and non-ASCII characters, launches
`org.weft.demo.notes` and edits it with real keyboard input:

1. Typing at the end and pressing Ctrl+S must store the seeded text plus
   the typed text, byte for byte; a load or save that rewrote `\\n`, quotes
   or tabs fails.
2. Text typed while a save is in flight (the runtime is paused with SIGSTOP
   between Ctrl+S and the reply) must still be in the editor afterwards: a
   second Ctrl+S stores it too.
3. After the file is changed on disk (as by another Notes window), a save
   must be refused and the other window's text kept.
4. Discard reloads the stored text. Keys typed while that reload is in
   flight (runtime paused again) must be ignored, so a following save stores
   the reloaded text plus only what was typed afterwards.
5. A load that fails (the stored notes are not valid UTF-8) leaves Discard
   usable: after the file is repaired, Discard loads it and it can be edited
   and saved again.
6. Notes declares the `ask` close policy. With unsaved changes, a close
   request (TERMINATE_APP or Alt+F4) is declined and a prompt is shown: the
   session keeps running and Cancel keeps the text; "Save and close" stores
   the text before the app shell exits cleanly; "Close without saving"
   leaves the stored notes unchanged. A forced TERMINATE_APP ends a session
   whose close was declined at once, saving nothing. Without unsaved
   changes, Notes closes at once.

Requires Xvfb, xwd, ImageMagick's convert and xdotool.
"""

import argparse
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True
from check_counter import AppdClient, free_port, read_endpoint  # noqa: E402
from session import ROOT, Desktop, SessionError, bounding_box, missing_tools, parse_ppm  # noqa: E402

APP_ID = "org.weft.demo.notes"
SEED = 'path C:\\new\\table "quoted"\tcaf\u00e9 \u2603'


PAGE = (15, 15, 15)  # The Notes page's background, #0f0f0f.
# The Discard button, right-aligned in the header: its offset from the
# window's right edge, and its height on the desktop.
DISCARD_FROM_RIGHT, DISCARD_Y = 137, 68
# The row through the close prompt's buttons, below the header.
PROMPT_Y = DISCARD_Y + 48


def runtime_pid():
    """The process ID of the Notes session's weft-runtime."""
    found = subprocess.run(["pgrep", "-n", "-f", f"weft-runtime {APP_ID}"],
                           capture_output=True, text=True)
    if found.returncode != 0:
        raise AssertionError("no weft-runtime process for Notes")
    return int(found.stdout.split()[0])


def wait_for_file(path, expected, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.read_text(encoding="utf-8") == expected:
            return
        time.sleep(0.2)
    actual = path.read_text(encoding="utf-8") if path.exists() else None
    raise AssertionError(f"stored notes {actual!r}, expected {expected!r}")


def prompt_buttons(desktop, page):
    """The close prompt's buttons, left to right, as (left, right) columns,
    or None when the prompt is not shown."""
    width, _, pixels = parse_ppm(desktop.capture())
    runs, start = [], None
    for x in range(page[0], page[2] + 2):
        i = (PROMPT_Y * width + x) * 3
        background = x > page[2] or tuple(pixels[i:i + 3]) == PAGE
        if not background and start is None:
            start = x
        elif background and start is not None:
            runs.append((start, x - 1))
            start = None
    buttons = [r for r in runs if r[1] - r[0] >= 40]
    # Shown, the row holds the prompt's text and its three buttons, with the
    # page between them; hidden, it crosses the editor from edge to edge.
    if len(buttons) < 3 or buttons[-1][1] - buttons[0][0] > (page[2] - page[0]) * 0.8:
        return None
    return buttons[-3:]


def wait_for_prompt(desktop, page, shown, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        buttons = prompt_buttons(desktop, page)
        if (buttons is not None) == shown:
            return buttons
        time.sleep(0.3)
    raise AssertionError("the close prompt was " + ("not shown" if shown else "not dismissed"))


def launch(appd):
    appd.send({"type": "LAUNCH_APP", "app_id": APP_ID, "surface_id": 0})
    reply = appd.wait_for(lambda m: m.get("type") in ("APP_READY", "ERROR"), 120)
    if reply.get("type") != "APP_READY":
        raise AssertionError(f"Notes did not start: {reply}")
    return reply["session_id"]


def wait_for_state(appd, session_id, state, timeout):
    try:
        appd.wait_for(lambda m: m.get("type") == "APP_STATE" and m.get("state") == state
                      and m.get("session_id") == session_id, timeout)
    except TimeoutError:
        raise AssertionError(f"session {session_id} did not become {state}")


def wait_for_decline(desktop, session_id, timeout=10):
    marker = f"the application declined the close session_id={session_id}"
    deadline = time.monotonic() + timeout
    while marker not in desktop.log_text("appd"):
        if time.monotonic() > deadline:
            raise AssertionError(f"session {session_id} did not decline the close")
        time.sleep(0.2)


def stop_reason(desktop, session_id):
    marker = f"stopping session session_id={session_id} reason=\""
    for line in desktop.log_text("appd").splitlines():
        if marker in line:
            return line.split(marker, 1)[1].rsplit('"', 1)[0]
    return None


def check_close_policy(desktop, appd, page, act, notes, session_id):
    """Step 6, from a running Notes session whose text is saved."""
    def click(column):
        act("mousemove", "--sync", str((column[0] + column[1]) // 2), str(PROMPT_Y))
        act("click", "1")

    def type_at_end(text):
        act("mousemove", "--sync", "100", "400")
        act("click", "1")
        act("key", "ctrl+End")
        act("type", "--delay", "40", text)

    stored = notes.read_text(encoding="utf-8")
    type_at_end(" unsaved")
    # The page declines the close; the session keeps running.
    appd.send({"type": "TERMINATE_APP", "session_id": session_id})
    wait_for_decline(desktop, session_id)
    buttons = wait_for_prompt(desktop, page, True)
    click(buttons[2])  # Cancel
    wait_for_prompt(desktop, page, False)
    # The compositor's close (Alt+F4) asks the page the same way.
    act("key", "alt+F4")
    buttons = wait_for_prompt(desktop, page, True)
    time.sleep(1)
    if stop_reason(desktop, session_id) is not None:
        raise AssertionError("the session stopped although the close was cancelled")
    click(buttons[0])  # Save and close
    wait_for_state(appd, session_id, "stopped", 20)
    if notes.read_text(encoding="utf-8") != stored + " unsaved":
        raise AssertionError("Save and close did not store the text before closing")
    reason = stop_reason(desktop, session_id)
    if reason is None or not reason.startswith("closed on request; app shell exited"):
        raise AssertionError(f"Notes did not exit cleanly after saving: {reason}")

    # Close without saving keeps the stored notes.
    session_id = launch(appd)
    time.sleep(2)
    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    type_at_end(" dropped")
    appd.send({"type": "TERMINATE_APP", "session_id": session_id})
    wait_for_decline(desktop, session_id)
    buttons = wait_for_prompt(desktop, page, True)
    click(buttons[1])  # Close without saving
    wait_for_state(appd, session_id, "stopped", 20)
    if notes.read_text(encoding="utf-8") != stored + " unsaved":
        raise AssertionError("closing without saving changed the stored notes")

    # A forced stop ends a session that declined its close, without asking.
    session_id = launch(appd)
    time.sleep(2)
    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    type_at_end(" forced")
    appd.send({"type": "TERMINATE_APP", "session_id": session_id})
    wait_for_decline(desktop, session_id)
    wait_for_prompt(desktop, page, True)
    forced = time.monotonic()
    appd.send({"type": "TERMINATE_APP", "session_id": session_id, "force": True})
    wait_for_state(appd, session_id, "stopped", 10)
    if time.monotonic() - forced > 3:
        raise AssertionError("a forced stop waited for the application")
    if stop_reason(desktop, session_id) != "terminated by force":
        raise AssertionError(f"a forced stop reported {stop_reason(desktop, session_id)!r}")
    if notes.read_text(encoding="utf-8") != stored + " unsaved":
        raise AssertionError("a forced stop changed the stored notes")

    # Without unsaved changes, a close request closes Notes at once.
    session_id = launch(appd)
    time.sleep(2)
    appd.send({"type": "TERMINATE_APP", "session_id": session_id})
    wait_for_state(appd, session_id, "stopped", 10)
    reason = stop_reason(desktop, session_id)
    if reason is None or not reason.startswith("closed on request; app shell exited"):
        raise AssertionError(f"Notes without changes did not close on request: {reason}")


def run(args, desktop, home, xdotool):
    data_home = home / "share"
    notes = data_home / "weft/app-data" / APP_ID / "notes.txt"
    notes.parent.mkdir(parents=True)
    notes.write_text(SEED, encoding="utf-8")

    t = Path(args.target)
    desktop.launch_client("appd", [t / "weft-appd"], {
        "WEFT_RUNTIME_BIN": str(t / "weft-runtime"),
        "WEFT_APP_SHELL_BIN": str(t / "weft-app-shell"),
        "WEFT_APP_STORE": str(args.store),
        "WEFT_DISABLE_CGROUP": "1",
        "WEFT_APPD_WS_PORT": str(free_port()),
        "HOME": str(home),
        "XDG_DATA_HOME": str(data_home),
    })
    port, token = read_endpoint(desktop.runtime, 30)
    appd = AppdClient(port, token)
    session_id = launch(appd)
    time.sleep(2)
    width, height, pixels = parse_ppm(desktop.capture())
    page = bounding_box(width, height, pixels, PAGE)
    if page is None:
        raise AssertionError("the Notes page is not on screen")
    discard = (str(page[2] + 1 - DISCARD_FROM_RIGHT), str(DISCARD_Y))

    def act(*arguments):
        subprocess.run([xdotool, *arguments], env={"DISPLAY": desktop.display},
                       check=True, timeout=30)

    act("search", "--class", "weft-compositor", "windowfocus", "--sync")
    # The editor fills the window below the header.
    act("mousemove", "--sync", "100", "300")
    act("click", "1")
    act("key", "ctrl+End")

    runtime = runtime_pid()

    act("type", "--delay", "40", ' +\\t "x"')
    expected = SEED + ' +\\t "x"'
    os.kill(runtime, signal.SIGSTOP)
    try:
        act("key", "ctrl+s")
        act("type", "--delay", "40", " more")
        time.sleep(2)
    finally:
        os.kill(runtime, signal.SIGCONT)
    wait_for_file(notes, expected)
    time.sleep(1)
    act("key", "ctrl+s")
    expected += " more"
    wait_for_file(notes, expected)

    other = "written by another window"
    notes.write_text(other, encoding="utf-8")
    act("type", "--delay", "40", " lost?")
    act("key", "ctrl+s")
    time.sleep(3)
    if notes.read_text(encoding="utf-8") != other:
        raise AssertionError("a save replaced notes changed elsewhere")

    os.kill(runtime, signal.SIGSTOP)
    try:
        act("mousemove", "--sync", *discard)
        act("click", "1")
        act("mousemove", "--sync", "100", "300")
        act("click", "1")
        act("type", "--delay", "40", "zz")
        # Let the page handle the keys while the reload is still unanswered.
        time.sleep(2)
    finally:
        os.kill(runtime, signal.SIGCONT)
    time.sleep(2)
    act("key", "ctrl+End")
    act("type", "--delay", "40", "!")
    act("key", "ctrl+s")
    wait_for_file(notes, other + "!")
    notes.write_bytes(b"\xff not text")
    act("mousemove", "--sync", *discard)
    act("click", "1")
    time.sleep(2)
    # The editor stays read-only and nothing can be saved over the file.
    act("mousemove", "--sync", "100", "300")
    act("click", "1")
    act("type", "--delay", "40", "x")
    act("key", "ctrl+s")
    time.sleep(2)
    if notes.read_bytes() != b"\xff not text":
        raise AssertionError("a failed load changed the stored notes")
    act("mousemove", "--sync", *discard)
    notes.write_text("repaired", encoding="utf-8")
    act("click", "1")
    time.sleep(2)
    act("mousemove", "--sync", "100", "300")
    act("click", "1")
    act("key", "ctrl+End")
    act("type", "--delay", "40", "?")
    act("key", "ctrl+s")
    wait_for_file(notes, "repaired?")

    leftovers = [p.name for p in notes.parent.iterdir() if p.name != "notes.txt"]
    if leftovers:
        raise AssertionError(f"files left beside the notes: {leftovers}")

    check_close_policy(desktop, appd, page, act, notes, session_id)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/debug/weft-compositor")
    parser.add_argument("--target", type=Path, default=ROOT / "target/debug",
                        help="directory with weft-appd, weft-runtime and weft-app-shell")
    parser.add_argument("--store", type=Path, default=ROOT / "examples",
                        help="app store containing org.weft.demo.notes")
    parser.add_argument("--output", type=Path, default=ROOT / "target/notes-check")
    args = parser.parse_args(argv)

    xdotool = shutil.which("xdotool")
    missing = missing_tools() + ([] if xdotool else ["xdotool"])
    missing += [str(p) for p in (args.compositor, args.target / "weft-appd",
                                 args.target / "weft-runtime", args.target / "weft-app-shell")
                if not p.is_file()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 2
    home = Path(tempfile.mkdtemp(prefix="weft-home-"))
    try:
        with Desktop(args.compositor, args.output, outputs=["appd.log", "notes.ppm"]) as desktop:
            try:
                run(args, desktop, home, xdotool)
            except AssertionError:
                (args.output / "notes.ppm").write_bytes(desktop.capture())
                raise
    except (AssertionError, SessionError, TimeoutError) as failure:
        print(f"notes check failed: {failure}", file=sys.stderr)
        print(f"logs and capture in {args.output}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(home, ignore_errors=True)
    print("notes check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
