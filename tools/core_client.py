"""A minimal frontend for quick-launcher-core, speaking the public
frontend protocol (docs/protocol.md, *Frontend protocol*) over the core's
stdin and stdout.

`tools/dev.py` drives it from a terminal and `tests/e2e_core.py` from
tests. Only the standard library is used: being a second frontend, in
another language than the shell's, is the point.
"""

import json
import queue
import subprocess
import threading

VERSION = 1


class CoreClient:
    """A running core and the messages it sent.

    A reader thread queues each message from the core. `recv` takes them
    in order and keeps `state` current, following the stale-state rule:
    a state whose `seq` is older than the last `input` sent updates the
    items but not `mode` or `text`.
    """

    def __init__(self, argv, env=None, stderr=None):
        self.process = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
            env=env,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self.seq = 0
        self.mode = None
        self.text = ""
        self.state = None
        # dev.py writes from its reader thread too (`open` after `done`).
        self._write_lock = threading.Lock()
        self._messages = queue.Queue()
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self):
        for line in self.process.stdout:
            if line.strip():
                self._messages.put(json.loads(line))
        self._messages.put(None)

    def send(self, kind, **fields):
        self.write({"v": VERSION, "type": kind, **fields})

    def write(self, message):
        """Sends one message as given, `v` and `type` included."""
        with self._write_lock:
            self.process.stdin.write(json.dumps(message) + "\n")
            self.process.stdin.flush()

    def hello(self, max_items=None, v=VERSION):
        fields = {"client": "core_client.py", "client_version": "0"}
        if max_items is not None:
            fields["max_items"] = max_items
        self.write({"v": v, "type": "hello", **fields})

    def open(self, mode=None):
        self.send("open", **({"mode": mode} if mode else {}))

    def input(self, text):
        self.seq += 1
        self.text = text
        self.send("input", seq=self.seq, mode=self.mode, text=text)

    def complete(self, item=None):
        self.send("complete", **({"item": item} if item else {}))

    def activate(self, item, action):
        self.send("activate", item=item, action=action)

    def recv(self, timeout=10):
        """The next message, or `None` once the core closed stdout. Raises
        `TimeoutError` when nothing arrives in time."""
        try:
            message = self._messages.get(timeout=timeout)
        except queue.Empty:
            raise TimeoutError(f"no message from the core within {timeout} s") from None
        if message is not None and message.get("type") == "state":
            self.state = message
            if message["seq"] >= self.seq:
                self.mode = message["mode"]["id"] if message["mode"] else None
                self.text = message["text"]
        return message

    def wait_for(self, predicate, timeout=10):
        """Messages until one matches `predicate`, which is returned."""
        while True:
            message = self.recv(timeout)
            if message is None:
                raise EOFError("the core closed stdout")
            if predicate(message):
                return message

    def settled_state(self, timeout=10):
        """The next state with no plugin request outstanding."""
        return self.wait_for(lambda m: m["type"] == "state" and not m["loading"], timeout)

    def finish(self, timeout=10):
        """Closes the core's stdin and returns its exit status."""
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass
        try:
            return self.process.wait(timeout)
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self._reader.join(timeout)
            self.process.stdout.close()
