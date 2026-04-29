#!/usr/bin/env python3
"""Launch prism GUI, then build a small graph from a remote script with
visible delays between steps so you can watch each action land.

Layout:

    [get_a] ─┐
             ├──> [sum] ──> [print]
    [get_b] ─┘

The script also batches a few finishing actions (selecting + renaming the
sum node) into a single `apply_all([...])` call to demonstrate atomic
multi-action undo: one Ctrl-Z reverts the whole batch. Then it animates
the sum node along a sine wave by streaming `move_node()` calls one
frame at a time — the GUI redraws between each because each `apply()`
fires the Notify wakeup the script crate handed Session.

All animation frames coalesce into one undo entry: `MoveNode`'s
gesture key is `NodeDrag(node_id)`, so consecutive moves of the same
node merge in the undo stack. One Ctrl-Z undoes the entire wave.

Run from the repo root:
    python3 prism/examples/demo_gui_build_graph.py
"""

import json
import math
import signal
import socket
import struct
import subprocess
import time
import uuid

import lz4.block  # pip install lz4 — wire-compatible with lz4_flex block format

PORT = 34567
TOKEN = uuid.UUID("550e8400-e29b-41d4-a716-446655440000")
STEP_DELAY = 1.5  # seconds between actions, so the canvas reads as a "build"
INITIAL_PAUSE = 1.5  # let the GUI show the empty canvas first


def send(sock, source, session_id=None):
    sid = session_id.bytes if session_id is not None else b"\x00" * 16
    body = lz4.block.compress(source)  # store_size=True by default
    sock.sendall(sid + struct.pack(">I", len(body)) + body)
    raw_len = recv_exact(sock, 4)
    (body_len,) = struct.unpack(">I", raw_len)
    return json.loads(lz4.block.decompress(recv_exact(sock, body_len)))


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


def step(sock, label, source, session_id, *, sleep=STEP_DELAY):
    """Run one Rhai snippet, log the result, then pause."""
    reply = send(sock, source, session_id)
    if reply.get("error"):
        print(f"  [{label}]  ERROR  {reply['error']}")
    else:
        result = reply.get("result", "").rstrip()
        print(f"  [{label}]  {result}")
    time.sleep(sleep)
    return reply


def main():
    prism = subprocess.Popen(
        [
            "cargo",
            "run",
            "--quiet",
            "--bin",
            "prism",
            "--",
            "--script-tcp",
            "--script-bind",
            f":{PORT}",
            "--script-token",
            str(TOKEN),
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

        # Frame 0: open a session and stash the func ids we'll need so
        # later frames don't have to re-query the lib. This is just a
        # bind step — no graph action yet.
        bootstrap = send(
            sock,
            (
                b"let funcs = list_funcs();"
                b' let get_a   = funcs.filter(|f| f.name == "get_a")[0].id;'
                b' let get_b   = funcs.filter(|f| f.name == "get_b")[0].id;'
                b' let sum_fn  = funcs.filter(|f| f.name == "sum")[0].id;'
                b' let print_fn= funcs.filter(|f| f.name == "print")[0].id;'
                b' "ready"'
            ),
            session_id=None,
        )
        session_id = uuid.UUID(bootstrap["session"])
        print(f"session {session_id}; pausing {INITIAL_PAUSE}s before building graph…")
        time.sleep(INITIAL_PAUSE)

        # Place the four nodes one by one. `create_node` returns the new
        # node id as a string — we save each into the Rhai scope so the
        # connection step can reference it.
        print("creating nodes:")
        step(
            sock,
            "get_a",
            b"let a_id = create_node(get_a, 100.0, 100.0); a_id",
            session_id,
        )
        step(
            sock,
            "get_b",
            b"let b_id = create_node(get_b, 100.0, 220.0); b_id",
            session_id,
        )
        step(
            sock,
            "sum",
            b"let s_id = create_node(sum_fn, 320.0, 160.0); s_id",
            session_id,
        )
        step(
            sock,
            "print",
            b"let p_id = create_node(print_fn, 540.0, 160.0); p_id",
            session_id,
        )

        # Connections via the prelude `connect()` helper. No need to
        # specify a "before" value — Session captures the previous
        # binding automatically when the action lands.
        print("wiring connections:")
        step(sock, "get_a → sum.input[0]", b"connect(a_id, 0, s_id, 0)", session_id)
        step(sock, "get_b → sum.input[1]", b"connect(b_id, 0, s_id, 1)", session_id)
        step(sock, "sum → print.input[0]", b"connect(s_id, 0, p_id, 0)", session_id)

        # Bulk action demo: select the sum node and rename it, atomically.
        # `apply_all([...])` ships the whole batch as a single
        # `SessionInbound::Apply(Vec<...>)`, which lands as one undo step.
        print("bulk: select + rename in one undo step…")
        step(
            sock,
            "apply_all",
            b"apply_all(["
            b"   #{ SelectNode: #{ to: s_id } },"
            b'   #{ RenameNode: #{ node_id: s_id, to: "a + b" } },'
            b"])",
            session_id,
        )

        # Animation: stream `move_node` calls so each frame paints between
        # network round-trips. Two cycles of a horizontal sine wave around
        # the sum node's home position, ending exactly back there.
        print("animating sum node…")
        cx, cy = 320.0, 160.0
        amplitude = 80.0
        cycles = 2
        frames = 60
        duration = 2.5  # seconds
        frame_dt = duration / frames
        for i in range(frames):
            t = i / (frames - 1)  # 0.0 → 1.0
            x = cx + amplitude * math.sin(t * cycles * 2 * math.pi)
            src = f"move_node(s_id, {x:.2f}, {cy})".encode()
            send(sock, src, session_id)
            time.sleep(frame_dt)
        # Snap to the canonical home in case rounding drifted us off.
        send(sock, f"move_node(s_id, {cx}, {cy})".encode(), session_id)

        print("done — try Ctrl-Z in the GUI; the rename+select should undo together")
        print("Ctrl-C here (or close the window) to exit")
        sock.close()
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
