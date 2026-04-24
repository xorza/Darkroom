#!/usr/bin/env python3
"""Launch prism TUI with a known TCP listener, send scripts over a persistent
connection, demonstrate session resume.

Run from the repo root:
    python3 prism/examples/demo_client.py

Demonstrates three things:
  1. First frame: session_id = nil ("new session"). Server returns a fresh id.
  2. Second frame on the same connection: resume that session, read back a
     variable bound in frame 1.
  3. Close the socket, open a new one, resume the *same* session id to show
     the scope survived a reconnect.
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


def send_request(sock: socket.socket, session_id: uuid.UUID | None, source: bytes) -> None:
    """Wire format: [16B session-id][u32 BE src_len][src_bytes]."""
    sid_bytes = session_id.bytes if session_id is not None else b"\x00" * 16
    sock.sendall(sid_bytes)
    sock.sendall(struct.pack(">I", len(source)) + source)


def read_reply(sock: socket.socket) -> dict:
    """Read one [u32 BE body_len][JSON] reply frame."""
    raw_len = recv_exact(sock, 4)
    (body_len,) = struct.unpack(">I", raw_len)
    body = recv_exact(sock, body_len)
    return json.loads(body)


def recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError(f"connection closed after {len(buf)}/{n} bytes")
        buf += chunk
    return bytes(buf)


def connect_and_auth() -> socket.socket:
    """Open a new connection and send the 16-byte auth token."""
    sock = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    sock.sendall(TOKEN.bytes)
    return sock


def main() -> None:
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
        start_new_session=True,
    )
    os.close(slave)

    try:
        # Wait for the listener to bind.
        deadline = time.time() + 20
        first = None
        while time.time() < deadline:
            try:
                first = connect_and_auth()
                break
            except (ConnectionRefusedError, OSError):
                time.sleep(0.1)
        if first is None:
            raise TimeoutError(f"prism never listened on {PORT}")

        # Frame 1: start a session, print + bind x.
        send_request(first, None, b'print("hello"); let x = 42;')
        reply1 = read_reply(first)
        session_id = uuid.UUID(reply1["session"])
        print(f"frame 1  session={session_id}")
        print(f"         print ={reply1['print']!r}")
        print(f"         result={reply1['result']!r}")

        # Frame 2 on the same connection: resume, read x back.
        send_request(first, session_id, b"x + 1")
        reply2 = read_reply(first)
        print(f"frame 2  session={reply2['session']}")
        print(f"         result={reply2['result']!r}")

        # Close the socket, reopen — same session id, state survives.
        first.close()
        second = connect_and_auth()
        send_request(second, session_id, b'"still alive: " + x')
        reply3 = read_reply(second)
        print(f"after reconnect  session={reply3['session']}")
        print(f"                 result ={reply3['result']!r}")
        second.close()

    finally:
        prism.send_signal(signal.SIGTERM)
        try:
            prism.wait(timeout=5)
        except subprocess.TimeoutExpired:
            prism.kill()
        os.close(master)


if __name__ == "__main__":
    main()
