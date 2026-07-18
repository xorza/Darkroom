#!/usr/bin/env python3
"""Build a small graph in the darkroom GUI from a remote script over the
script-TCP transport, with visible delays between steps so you can watch
each action land on the canvas.

Layout:

    [random] ─┐
              ├──> [add] ──> [print]
    [random] ─┘

The script also batches a couple of finishing actions (selecting + renaming
the add node) into a single `apply_all([...])` call to demonstrate atomic
multi-action undo: one Ctrl-Z reverts the whole batch. Then it animates the
add node along a sine wave by streaming `move_node()` calls one frame at a
time — the GUI redraws between each because every applied intent fires the
wake the script host handed the editor.

All animation frames coalesce into one undo entry: `MoveNodes`' gesture key
is `NodeDrag(node_id)`, so consecutive moves of the same node merge in the
undo stack. One Ctrl-Z undoes the entire wave.

Requires `pip install lz4` (wire-compatible with the `lz4_flex` block format
the transport uses). Run from the repo root:

    # attach to an already-running, no-auth instance (e.g. the Zed task):
    python3 darkroom/examples/demo_gui_build_graph.py --attach 127.0.0.1:34567

    # or launch a throwaway token-auth instance and drive it:
    python3 darkroom/examples/demo_gui_build_graph.py
"""

import argparse
import math
import json
import signal
import socket
import struct
import subprocess
import time
import uuid

import lz4.block  # pip install lz4 — wire-compatible with lz4_flex block format

DEFAULT_PORT = 34567
STEP_DELAY = 1.5  # seconds between actions, so the canvas reads as a "build"
INITIAL_PAUSE = 1.5  # let the GUI settle before building


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


def connect(host, port, token):
    """Open a connection and (when `token` is set) present the auth token."""
    s = socket.create_connection((host, port), timeout=5)
    if token is not None:
        s.sendall(token.bytes)
    return s


def connect_retry(host, port, token, deadline):
    while time.time() < deadline:
        try:
            return connect(host, port, token)
        except (ConnectionRefusedError, OSError):
            time.sleep(0.1)
    raise TimeoutError(f"nothing listening on {host}:{port}")


def step(sock, label, source, session_id, *, sleep=STEP_DELAY):
    """Run one Rhai snippet, log the result, then pause."""
    reply = send(sock, source, session_id)
    if reply.get("error"):
        print(f"  [{label}]  ERROR  {reply['error']}")
    else:
        print(f"  [{label}]  {json.dumps(reply.get('result'), ensure_ascii=False)}")
    time.sleep(sleep)
    return reply


def build_and_animate(sock):
    # Frame 0: open a session and stash the func ids we'll need so later
    # frames don't re-query the lib. Two `random` nodes stand in as sources.
    bootstrap = send(
        sock,
        (
            b"let funcs = list_funcs();"
            b' let rand_fn = funcs.filter(|f| f.name == "random")[0].id;'
            b' let add_fn  = funcs.filter(|f| f.name == "add")[0].id;'
            b' let print_fn= funcs.filter(|f| f.name == "print")[0].id;'
            b' "ready"'
        ),
        session_id=None,
    )
    if bootstrap.get("error"):
        raise RuntimeError(f"bootstrap failed: {bootstrap['error']}")
    session_id = uuid.UUID(bootstrap["session"])
    print(f"session {session_id}; pausing {INITIAL_PAUSE}s before building graph…")
    time.sleep(INITIAL_PAUSE)

    # Place the four nodes one by one. `create_node` returns the new node id
    # as a string — saved into the Rhai scope so later steps can reference it.
    print("creating nodes:")
    step(sock, "random (a)", b"let a_id = create_node(rand_fn, 100.0, 100.0); a_id", session_id)
    step(sock, "random (b)", b"let b_id = create_node(rand_fn, 100.0, 220.0); b_id", session_id)
    step(sock, "add", b"let s_id = create_node(add_fn, 320.0, 160.0); s_id", session_id)
    step(sock, "print", b"let p_id = create_node(print_fn, 540.0, 160.0); p_id", session_id)

    # Connections via the prelude `connect()` helper. No "before" value
    # needed — the editor captures the previous binding when the action lands.
    print("wiring connections:")
    step(sock, "a → add.input[0]", b"connect(a_id, 0, s_id, 0)", session_id)
    step(sock, "b → add.input[1]", b"connect(b_id, 0, s_id, 1)", session_id)
    step(sock, "add → print.input[0]", b"connect(s_id, 0, p_id, 0)", session_id)

    # Bulk action demo: select the add node and rename it, atomically.
    # `apply_all([...])` ships the batch as a single `Apply(Vec<...>)`, which
    # lands as one undo step.
    print("bulk: select + rename in one undo step…")
    step(
        sock,
        "apply_all",
        b"apply_all(["
        b"   #{ SetSelection: #{ to: [s_id] } },"
        b'   #{ RenameNode: #{ node_id: s_id, to: "a + b" } },'
        b"])",
        session_id,
    )

    # Animation: stream `move_node` calls so each frame paints between network
    # round-trips. Two cycles of a horizontal sine wave around the add node's
    # home, ending exactly back there.
    print("animating add node…")
    cx, cy = 320.0, 160.0
    amplitude, cycles, frames, duration = 80.0, 2, 60, 2.5
    frame_dt = duration / frames
    for i in range(frames):
        t = i / (frames - 1)  # 0.0 → 1.0
        x = cx + amplitude * math.sin(t * cycles * 2 * math.pi)
        send(sock, f"move_node(s_id, {x:.2f}, {cy})".encode(), session_id)
        time.sleep(frame_dt)
    send(sock, f"move_node(s_id, {cx}, {cy})".encode(), session_id)  # snap home


def parse_args():
    p = argparse.ArgumentParser(description="Build + animate a graph in darkroom over script-TCP.")
    p.add_argument(
        "--attach",
        metavar="HOST:PORT",
        help="drive an already-running darkroom instead of launching one",
    )
    p.add_argument(
        "--token",
        metavar="UUID",
        help="auth token to present (omit for a --script-no-auth server)",
    )
    return p.parse_args()


def main():
    args = parse_args()

    if args.attach:
        host, _, port = args.attach.partition(":")
        port = int(port) if port else DEFAULT_PORT
        token = uuid.UUID(args.token) if args.token else None
        proc = None
    else:
        # Launch a throwaway token-auth instance we own.
        host, port = "127.0.0.1", DEFAULT_PORT
        token = uuid.uuid4()
        proc = subprocess.Popen(
            ["cargo", "run", "--quiet", "-p", "darkroom", "--",
             "--script-tcp", "--script-bind", f":{port}",
             "--script-token", str(token), "gui"],
            start_new_session=True,
        )

    try:
        sock = connect_retry(host, port, token, deadline=time.time() + 60)
        build_and_animate(sock)
        sock.close()
        print("done — try Ctrl-Z in the GUI: rename+select undo together, then the wave")
        if proc is not None:
            print("Ctrl-C here (or close the window) to exit")
            proc.wait()
    except KeyboardInterrupt:
        pass
    finally:
        if proc is not None and proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    main()
