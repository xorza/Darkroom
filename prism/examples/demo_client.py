#!/usr/bin/env python3
"""Launch prism TUI with a known TCP listener, send a Rhai script, read reply.

Run from the repo root:
    python3 prism/examples/demo_client.py

Expected output:
    print:   'hello from python\\n'
    result:  '#{\\n    count: 3,\\n    msg: "ok",\\n}\\n'
    error:   None
"""

import json
import os
import pty
import signal
import socket
import struct
import subprocess
import time
import uuid

PORT = 34567
TOKEN = uuid.UUID("550e8400-e29b-41d4-a716-446655440000")


def main() -> None:
    # 1) Launch prism in TUI mode. Give it a pty so crossterm's raw-mode
    #    init succeeds — we don't read the master side, it just keeps the
    #    TUI happy while Python owns the real terminal.
    master, slave = pty.openpty()
    prism = subprocess.Popen(
        [
            "cargo", "run", "--quiet", "--bin", "prism", "--",
            "--script-tcp",
            "--script-bind", f":{PORT}",
            "--script-token", str(TOKEN),
            "tui",
        ],
        stdin=slave, stdout=slave, stderr=slave,
        close_fds=True,
        start_new_session=True,  # so SIGTERM can kill the whole group
    )
    os.close(slave)

    try:
        # 2) Wait for the TCP listener — retry with backoff until it binds.
        deadline = time.time() + 20
        sock = None
        while time.time() < deadline:
            try:
                sock = socket.create_connection(("127.0.0.1", PORT), timeout=1)
                break
            except (ConnectionRefusedError, OSError):
                time.sleep(0.1)
        if sock is None:
            raise TimeoutError(f"prism never listened on {PORT}")

        # 3) Send 16-byte UUID token (u128 BE) + framed script.
        sock.sendall(TOKEN.bytes)
        script = b'print("hello from python"); #{ msg: "ok", count: 3 }'
        sock.sendall(struct.pack(">I", len(script)) + script)

        # 4) Read framed JSON reply.
        (reply_len,) = struct.unpack(">I", recv_exact(sock, 4))
        body = recv_exact(sock, reply_len)
        reply = json.loads(body)

        print("print:  ", repr(reply["print"]))
        print("result: ", repr(reply["result"]))
        print("error:  ", repr(reply["error"]))
        sock.close()

    finally:
        # 5) Shut prism down.
        prism.send_signal(signal.SIGTERM)
        try:
            prism.wait(timeout=5)
        except subprocess.TimeoutExpired:
            prism.kill()
        os.close(master)


def recv_exact(sock: socket.socket, n: int) -> bytes:
    """Read exactly `n` bytes or raise — socket.recv may return fewer."""
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError(f"connection closed after {len(buf)}/{n} bytes")
        buf += chunk
    return bytes(buf)


if __name__ == "__main__":
    main()
