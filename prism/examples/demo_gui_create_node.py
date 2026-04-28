#!/usr/bin/env python3
"""Launch prism GUI, wait a few seconds, then drive a script that adds a
`print` node to the live graph at view-space (100, 100).

Run from the repo root:
    python3 prism/examples/demo_gui_create_node.py

The prism window stays open until you close it (or until SIGINT here).
Watch the canvas: the new node should appear after the pause.
"""

import json
import signal
import socket
import struct
import subprocess
import time
import uuid

PORT = 34567
TOKEN = uuid.UUID("550e8400-e29b-41d4-a716-446655440000")
PAUSE_BEFORE_SCRIPT = 3.0


def send_request(sock, session_id, source):
    sid = session_id.bytes if session_id is not None else b"\x00" * 16
    sock.sendall(sid + struct.pack(">I", len(source)) + source)


def read_reply(sock):
    raw_len = recv_exact(sock, 4)
    (body_len,) = struct.unpack(">I", raw_len)
    return json.loads(recv_exact(sock, body_len))


def recv_exact(sock, n):
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError(f"closed after {len(buf)}/{n}")
        buf += chunk
    return bytes(buf)


def connect_and_auth():
    s = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    s.sendall(TOKEN.bytes)
    return s


def main():
    # No pty: GUI opens its own window; we just inherit stdio.
    prism = subprocess.Popen(
        [
            "cargo", "run", "--quiet", "--bin", "prism", "--",
            "--script-tcp",
            "--script-bind", f":{PORT}",
            "--script-token", str(TOKEN),
            # `gui` is the default subcommand, but be explicit.
            "gui",
        ],
        start_new_session=True,
    )

    try:
        # Wait for the script listener to bind.
        deadline = time.time() + 30
        sock = None
        while time.time() < deadline:
            try:
                sock = connect_and_auth()
                break
            except (ConnectionRefusedError, OSError):
                time.sleep(0.1)
        if sock is None:
            raise TimeoutError(f"prism never listened on {PORT}")

        print(f"connected; pausing {PAUSE_BEFORE_SCRIPT}s so the GUI is visible…")
        time.sleep(PAUSE_BEFORE_SCRIPT)

        # Resolve the `print` func id off the live FuncLib and create a
        # node at (100, 100). `filter` lets us survive the duplicate
        # "print" entries (test fixture + BasicFuncLib both register one)
        # — we just take the first match.
        script = (
            b'let f = list_funcs().filter(|f| f.name == "print")[0];'
            b' create_node(f.id, 100.0, 100.0)'
        )
        send_request(sock, None, script)
        reply = read_reply(sock)
        print(f"create_node print  result ={reply['result']!r}")
        print(f"                   error  ={reply['error']!r}")
        sock.close()

        print("node added; leave the GUI open as long as you like (Ctrl-C to quit)")
        prism.wait()

    except KeyboardInterrupt:
        pass
    finally:
        if prism.poll() is None:
            prism.send_signal(signal.SIGTERM)
            try:
                prism.wait(timeout=5)
            except subprocess.TimeoutExpired:
                prism.kill()


if __name__ == "__main__":
    main()
