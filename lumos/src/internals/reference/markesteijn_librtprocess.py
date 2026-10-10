"""Digests of librtprocess's Markesteijn on the scenes of `markesteijn_matches_librtprocess_bit_for_bit`.

Fetches librtprocess's `markesteijn.cc` at a pinned commit, builds it with a harness that
demosaics each scene on the X-Trans fixture with one pass and with three, and prints, per case,
the FNV-1a 64 digest of the output's interior: the three planes in order, rows from the pass
count's border to `HEIGHT` less it, the same columns, each f32's little-endian bytes. Outside that
band librtprocess reads colours it never computed, and lumos fills the border from neighbours.

The harness writes each native sample as the input holds it, as lumos does: librtprocess takes
it from the mean of the chosen directions, which rounds it where three or more are chosen. Every
interpolated sample is compared as librtprocess makes it.

Two lines are changed. The loop that fills red and blue for 2x2 blocks of green runs to the
direction count in steps of two, so one pass, of four directions, fills two and leaves the others'
red and blue at zero (LibRaw issue 441); the harness runs it over all four. And the tile is one
256-pixel tile over the whole frame: librtprocess's 114-pixel tiles overlap by 16, and each writes
all but 8 pixels at each side, nearer its edge than its passes compute in full, so the pixels
beside a seam differ from an untiled run. lumos's tiles write only what they compute in full.

The build takes librtprocess's scalar paths (`-U__SSE2__`): its SSE YPbPr swaps the green and blue
weights. It uses YPbPr for both pass counts (`useCieLab = false`), as lumos does. It disables
contraction: Rust never fuses a multiply and an add. The scenes use only correctly rounded
operations, so their samples are the same bits on every platform.

Run: python3 markesteijn_librtprocess.py (needs g++ and network access).
"""

import array
import pathlib
import subprocess
import sys
import tempfile
import urllib.request

COMMIT = "9a858270acb2096e2e403d932760ee688fcac425"
FILES = [
    "src/demosaic/markesteijn.cc",
    "src/include/librtprocess.h",
    "src/include/rt_math.h",
    "src/include/opthelper.h",
    "src/include/LUT.h",
    "src/include/sleef.h",
    "src/include/xtranshelper.h",
]
WIDTH, HEIGHT = 160, 120
# Per pass count: 3, where librtprocess's first tile starts, plus lumos's margin.
BORDERS = {1: 12, 3: 18}

CHANGES = [
    (
        "for (int d = 0; d < ndir; d += 2, rix += ts * ts) {",
        "for (int d = 0; d < 8; d += 2, rix += ts * ts) {",
    ),
    ("constexpr int ts = 114;", "constexpr int ts = 256;"),
]

STUBS = {
    "StopWatch.h": "#pragma once\n#include <iostream>\n#include <string>\n#define BENCHFUN\nstruct StopWatch { explicit StopWatch(const char*) {} };\n",
    "sleefsseavx.h": "",
}

HARNESS = r"""
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <vector>
#include "librtprocess.h"

// The border is not compared, so librtprocess's border fill is not built.
void xtransborder_demosaic(int, int, int, const float *const *, float **, float **, float **, const unsigned[6][6]) {}

constexpr int W = WIDTH_, H = HEIGHT_;
// lumos's X-Trans fixture.
const unsigned PATTERN[6][6] = {{1,1,0,1,1,2},{1,1,2,1,1,0},{2,0,1,0,2,1},{1,1,2,1,1,0},{1,1,0,1,1,2},{0,2,1,2,0,1}};

float scene_value(int scene, int channel, int x, int y) {
    switch (scene) {
    case 0: {
        const float left[3] = {0.1f, 0.3f, 0.8f}, right[3] = {0.9f, 0.6f, 0.2f};
        return x < W / 2 ? left[channel] : right[channel];
    }
    case 1: {
        const float peak[3] = {1.0f, 0.7f, 0.4f};
        return (x == W / 2 && y == H / 2) ? peak[channel] : 0.05f;
    }
    case 2: {
        const float dx = float(x) - float(W - 1) * 0.5f, dy = float(y) - float(H - 1) * 0.5f;
        const float width[3] = {1.2f, 1.6f, 2.0f}, amplitude[3] = {0.9f, 0.7f, 0.5f};
        return 0.02f + amplitude[channel] / (1.0f + (dx * dx + dy * dy) / (width[channel] * width[channel]));
    }
    default: {
        const float phase[3] = {0.0f, 0.33333334f, 0.6666667f};
        const float t = 0.075f * float(x) + 0.05f * float(y) + phase[channel];
        return 0.1f + 1.6f * std::fabs(t - std::floor(t) - 0.5f);
    }
    }
}

int main() {
    const float rgb_cam[3][4] = {{1, 0, 0, 0}, {0, 1, 0, 0}, {0, 0, 1, 0}};
    for (int passes : {1, 3}) {
        for (int scene = 0; scene < 4; ++scene) {
            std::vector<float> raw(W * H), r(W * H), g(W * H), b(W * H);
            std::vector<const float *> rows(H);
            std::vector<float *> rr(H), gr(H), br(H);
            for (int y = 0; y < H; ++y) {
                for (int x = 0; x < W; ++x) raw[y * W + x] = scene_value(scene, PATTERN[y % 6][x % 6], x, y);
                rows[y] = &raw[y * W];
                rr[y] = &r[y * W]; gr[y] = &g[y * W]; br[y] = &b[y * W];
            }
            markesteijn_demosaic(W, H, rows.data(), rr.data(), gr.data(), br.data(), PATTERN, rgb_cam,
                                 [](double) { return false; }, passes, false, 1, false);
            // Each native sample as the input holds it, as lumos writes it.
            std::vector<float> *planes[3] = {&r, &g, &b};
            for (int y = 0; y < H; ++y)
                for (int x = 0; x < W; ++x) (*planes[PATTERN[y % 6][x % 6]])[y * W + x] = raw[y * W + x];
            for (auto *plane : planes) std::fwrite(plane->data(), sizeof(float), plane->size(), stdout);
        }
    }
}
"""


def fnv1a64(data: bytes) -> int:
    digest = 0xCBF29CE484222325
    for byte in data:
        digest = ((digest ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return digest


def main():
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        for file in FILES:
            url = f"https://raw.githubusercontent.com/CarVac/librtprocess/{COMMIT}/{file}"
            (root / pathlib.Path(file).name).write_bytes(urllib.request.urlopen(url).read())
        source = (root / "markesteijn.cc").read_text()
        for old, new in CHANGES:
            assert source.count(old) == 1, old
            source = source.replace(old, new)
        (root / "markesteijn.cc").write_text(source)
        for name, text in STUBS.items():
            (root / name).write_text(text)
        harness = HARNESS.replace("WIDTH_", str(WIDTH)).replace("HEIGHT_", str(HEIGHT))
        (root / "harness.cc").write_text(harness)
        subprocess.run(
            ["g++", "-std=c++17", "-O2", "-ffp-contract=off", "-U__SSE2__", "-I", ".", "harness.cc",
             "markesteijn.cc", "-o", "harness"],
            cwd=root,
            check=True,
        )
        output = subprocess.run([str(root / "harness")], capture_output=True, check=True).stdout
    samples = array.array("f")
    samples.frombytes(output)
    if sys.byteorder != "little":
        samples.byteswap()
    plane = WIDTH * HEIGHT
    for index, passes in enumerate([1, 3]):
        border = BORDERS[passes]
        row = []
        for scene in range(4):
            interior = array.array("f")
            for channel in range(3):
                start = ((index * 4 + scene) * 3 + channel) * plane
                for y in range(border, HEIGHT - border):
                    line = start + y * WIDTH
                    interior.extend(samples[line + border : line + WIDTH - border])
            if sys.byteorder != "little":
                interior.byteswap()
            row.append(f"0x{fnv1a64(interior.tobytes()):016x}")
        print(f"[{', '.join(row)}],")


if __name__ == "__main__":
    main()
