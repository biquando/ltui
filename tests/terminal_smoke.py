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
    def __init__(self, home, editor, *args):
        self.pid, self.fd = pty.fork()
        if not self.pid:
            env = dict(os.environ, HOME=str(home), EDITOR=editor, TERM="xterm-256color")
            env.pop("SUDO_UID", None)
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
                session.send(b"E")
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
    print("PTY smoke tests passed: full editor, invalid-edit recovery, form save/paste, terminal restoration")


if __name__ == "__main__":
    main()
