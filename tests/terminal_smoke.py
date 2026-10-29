"""macOS PTY integration test. Only temp files and read-only launchctl queries."""
import fcntl
import os
from pathlib import Path
import plistlib
import pty
import select
import shlex
import signal
import struct
import sys
import tempfile
import termios
import time

BINARY = Path(__file__).resolve().parents[1] / "target/debug/ltui"


class Session:
    def __init__(self, home, editor, *args, pager=None):
        self.pid, self.fd = pty.fork()
        if not self.pid:
            env = dict(os.environ, HOME=str(home), EDITOR=editor, TERM="xterm-256color")
            env.pop("SUDO_UID", None)
            if pager is not None:
                env["PAGER"] = pager
            os.execve(str(BINARY), [str(BINARY), *args], env)
        self.output = b""
        self.exited = False
        self.flags_before = termios.tcgetattr(self.fd)[3]
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))

    def drain(self, seconds=0.2):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([self.fd], [], [], 0.02)[0]:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                self.output += chunk

    def until(self, predicate):
        deadline = time.monotonic() + 10
        while not predicate():
            assert time.monotonic() < deadline, self.output.decode(errors="replace")[-5000:]
            self.drain()

    def send(self, data):
        os.write(self.fd, data)
        self.drain()

    def close(self):
        self.send(b"q")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.exited = True
                assert os.waitstatus_to_exitcode(status) == 0, self.output
                flags = termios.tcgetattr(self.fd)[3]
                assert (flags & (termios.ECHO | termios.ICANON)) == (
                    self.flags_before & (termios.ECHO | termios.ICANON)
                ), "terminal settings were not restored"
                assert b"\x1b[?1006h" in self.output, "SGR mouse capture was not enabled"
                assert b"\x1b[?1006l" in self.output, "mouse capture was not disabled"
                if self.output.count(b"\x1b[?1006h") > 1:
                    assert self.output.find(b"\x1b[?1006l") < self.output.rfind(b"\x1b[?1006h")
                return
            self.drain()
        raise AssertionError("app did not quit")

    def cleanup(self):
        if not self.exited:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def main():
    assert sys.platform == "darwin", "This integration test needs macOS"
    assert BINARY.exists(), "Run cargo build first"
    with tempfile.TemporaryDirectory(prefix="ltui-smoke-") as tmp:
        home = Path(tmp)
        directory = home / "Library/LaunchAgents"
        directory.mkdir(parents=True)
        path = directory / "local.ltui-smoke.plist"
        value = {
            "Label": f"local.ltui-smoke-{os.getpid()}",
            "ProgramArguments": ["/usr/bin/true"],
            "StartCalendarInterval": {"Hour": 9, "Minute": 30},
            "KeepAlive": {"SuccessfulExit": False},
        }
        path.write_bytes(plistlib.dumps(value, fmt=plistlib.FMT_BINARY))
        editor = home / "editor script.py"
        record = home / "edited-copy.txt"
        editor.write_text('''import sys, plistlib
from pathlib import Path
assert sys.argv[1] == "two words"
p = Path(sys.argv[-1])
if sys.argv[2] == "valid":
    v = plistlib.loads(p.read_bytes())
    v["ProgramArguments"] = ["/bin/echo", "two words", "$HOME"]
    p.write_bytes(plistlib.dumps(v))
else:
    p.write_text("not a plist")
Path(sys.argv[3]).write_text(str(p))
''')
        for mode in ["valid", "invalid"]:
            record.unlink(missing_ok=True)
            before = path.read_bytes()
            command = shlex.join([sys.executable, str(editor), "two words", mode, str(record)])
            session = Session(home, command)
            try:
                session.until(lambda: b"launchd job manager" in session.output)
                session.send(b"\r" if mode == "valid" else b" j\r")
                session.until(record.exists)
                if mode == "valid":
                    session.until(lambda: path.read_bytes() != before)
                    saved = plistlib.loads(path.read_bytes())
                    assert saved["ProgramArguments"] == ["/bin/echo", "two words", "$HOME"]
                    assert saved["StartCalendarInterval"] == value["StartCalendarInterval"]
                    assert saved["KeepAlive"] == value["KeepAlive"]
                else:
                    session.until(lambda: b"preserved" in session.output)
                    assert path.read_bytes() == before
                    recovery = Path(record.read_text())
                    assert recovery.read_text() == "not a plist"
                    session.send(b"\x1b")
                    recovery.unlink()
                session.close()
            finally:
                session.cleanup()
        # Open both configured logs through the file section of the action menu.
        pager_script = home / "pager script.py"
        pager_record = home / "pager-record.txt"
        pager_script.write_text('''import sys
from pathlib import Path
assert sys.argv[1] == "two words"
Path(sys.argv[2]).write_text(Path(sys.argv[-1]).read_text())
''')
        stdout = home / "stdout log.txt"
        stderr = home / "stderr log.txt"
        stdout.write_text("stdout content")
        stderr.write_text("stderr content")
        v = plistlib.loads(path.read_bytes())
        v.update(StandardOutPath=str(stdout), StandardErrorPath=str(stderr))
        path.write_bytes(plistlib.dumps(v))
        for index, log in [(2, stdout), (3, stderr)]:
            pager_record.unlink(missing_ok=True)
            pager = shlex.join([sys.executable, str(pager_script), "two words", str(pager_record)])
            session = Session(home, "vi", pager=pager)
            try:
                session.until(lambda: b"launchd job manager" in session.output)
                session.send(b" " + b"j" * index + b"\r")
                session.until(pager_record.exists)
                assert pager_record.read_text() == log.read_text()
                session.close()
            finally:
                session.cleanup()
        # Exercise the built-in form and bracketed paste without ever loading a job.
        session = Session(home, "vi")
        try:
            session.until(lambda: b"launchd job manager" in session.output)
            session.send(b"n")
            session.send(b"\x15local.form-smoke")  # clear Label, type a new one
            session.send(b"\t\t\x15")  # clear ProgramArguments
            script = "\nprintf 'hello world\\n'\necho \"$HOME\"\n"
            arguments = shlex.join(["/bin/bash", "-c", script])
            session.send(b"\x1b[200~" + arguments.encode() + b"\x1b[201~")
            session.send(b"\t" * 7)  # calendar schedule
            session.send(b"Hour=23 Minute=31\rWeekday=1 Hour=9 Minute=0")
            session.send(b"\t")  # environment variables
            session.send(b"MODE=dev MESSAGE='hello world'\rEMPTY=''")
            session.send(b"\x13")  # Ctrl-S
            created = directory / "local.form-smoke.plist"
            session.until(created.exists)
            saved = plistlib.loads(created.read_bytes())
            assert saved["ProgramArguments"] == ["/bin/bash", "-c", script]
            assert saved["StartCalendarInterval"] == [
                {"Hour": 23, "Minute": 31}, {"Weekday": 1, "Hour": 9, "Minute": 0}
            ]
            assert saved["EnvironmentVariables"] == {"MODE": "dev", "MESSAGE": "hello world", "EMPTY": ""}
            session.close()
        finally:
            session.cleanup()
        # SGR mouse reports: click existing controls and save only a temporary plist.
        def click(x, y, button=0):
            return f"\x1b[<{button};{x+1};{y+1}M\x1b[<{button};{x+1};{y+1}m".encode()

        session = Session(home, "vi")
        try:
            session.until(lambda: b"launchd job manager" in session.output)
            session.send(click(2, 39))  # Main footer is only a key guide.
            assert b"New job" not in session.output
            session.send(b"n")
            session.until(lambda: b"New job" in session.output)
            session.send(click(26, 5))  # Label value in the form
            session.send(b"\x15local.mouse-smoke")
            session.send(click(25, 9))  # ProgramArguments value
            session.send(b"\x15/usr/bin/true")
            session.send(click(24, 36))  # Form footer must not trigger Save.
            created = directory / "local.mouse-smoke.plist"
            assert not created.exists()
            session.send(b"\x13")
            session.until(created.exists)
            saved = plistlib.loads(created.read_bytes())
            assert saved["Label"] == "local.mouse-smoke"
            assert saved["ProgramArguments"] == ["/usr/bin/true"]
            session.send(click(4, 4))  # Search box
            session.send(b"local.mouse-smoke\r")
            session.send(click(5, 9, 2))  # Right-click the filtered job row
            session.until(lambda: b"Job actions" in session.output)
            session.send(b"\x1b")
            session.close()
        finally:
            session.cleanup()
        # A tab before a multibyte character must not produce a cursor inside
        # that character. Click the wide character, insert text, render, and save.
        v = plistlib.loads(path.read_bytes())
        v["WorkingDirectory"] = "\t界a"
        path.write_bytes(plistlib.dumps(v))
        session = Session(home, "vi")
        try:
            session.until(lambda: b"launchd job manager" in session.output)
            session.send(b"/" + v["Label"].encode() + b"\r")
            session.send(b"e")
            session.until(lambda: b"Edit job" in session.output)
            session.send(click(26, 11))  # Display column 2 of WorkingDirectory.
            session.send(b"X\x13")
            session.until(lambda: plistlib.loads(path.read_bytes())["WorkingDirectory"] == "\tX界a")
            assert b"panicked at" not in session.output
            session.close()
        finally:
            session.cleanup()
    print("PTY smoke tests passed: editor, log pager, mouse controls, form save/paste, capture and terminal restoration")


if __name__ == "__main__":
    main()
