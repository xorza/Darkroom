"""Digests of librtprocess's RCD on the scenes of `rcd_matches_librtprocess_bit_for_bit`.

Fetches librtprocess's `rcd.cc` at a pinned commit, builds it with a harness that demosaics each
scene in each Bayer phase, and prints, per case, the FNV-1a 64 digest of the output's interior: the
three planes in order, rows from `BORDER` to `HEIGHT - BORDER`, the same columns, each f32's
little-endian bytes. Outside that band librtprocess fills a border by its own interpolation.

Two steps are changed. RCD 2.3 (Luis Sanz Rodríguez, 2017) defines the diagonal statistics at a red
or blue site as the squared diagonal high-pass filter summed over the site and its two neighbours
along the diagonal; its closed forms expand exactly that sum. librtprocess, RawTherapee and darktable
keep the filter on odd columns only, in a half-width buffer, so on every row step 4.1 reads one or
two of the three from a pixel beside the diagonal, and which ones depends on the Bayer phase. The
harness computes the filter at every column and reads the three diagonal sites. And step 3's green
ratio `cfa·2c/(eps + c + s)` drops its `eps`, which biases faint signal by `eps/(c + s)`: lumos
takes the ratio level-free. The scenes are positive, so `c + s` never cancels. Every other step,
the `eps` that keeps the gradient weights finite among them, is librtprocess's own.

The frame fits in one of librtprocess's 194-pixel tiles. Its tiles overlap by 9 pixels, and RCD
reaches 10, so the two columns at a seam differ from an untiled run.

The scenes use only correctly rounded operations — no `exp` or `sin` — so their samples are the same
bits on every platform. The build disables contraction: Rust never fuses a multiply and an add.

Run: python3 rcd_librtprocess.py (needs g++ and network access).
"""

import array
import pathlib
import subprocess
import sys
import tempfile
import urllib.request

COMMIT = "9a858270acb2096e2e403d932760ee688fcac425"
FILES = [
    "src/demosaic/rcd.cc",
    "src/include/librtprocess.h",
    "src/include/rt_math.h",
    "src/include/opthelper.h",
    "src/include/bayerhelper.h",
]
WIDTH, HEIGHT, BORDER = 160, 120, 10

THREE_SITES = [
    (
        "float *const P_CDiff_Hpf = (float*) calloc(tileSize * tileSize / 2, sizeof *P_CDiff_Hpf);",
        "float *const P_CDiff_Hpf = (float*) calloc(tileSize * tileSize, sizeof *P_CDiff_Hpf);",
    ),
    (
        "float *const Q_CDiff_Hpf = (float*) calloc(tileSize * tileSize / 2, sizeof *Q_CDiff_Hpf);",
        "float *const Q_CDiff_Hpf = (float*) calloc(tileSize * tileSize, sizeof *Q_CDiff_Hpf);",
    ),
    (
        "for (int col = 3, indx = row * tileSize + col, indx2 = indx / 2; col < tilecols - 3; col+=2, indx+=2, indx2++ ) {",
        "for (int col = 3, indx = row * tileSize + col, indx2 = indx; col < tilecols - 3; col++, indx++, indx2++ ) {",
    ),
    (
        "float P_Stat = std::max(epssq, P_CDiff_Hpf[indx3] + P_CDiff_Hpf[indx2] + P_CDiff_Hpf[indx4 + 1]);",
        "float P_Stat = std::max(epssq, P_CDiff_Hpf[indx - w1 - 1] + P_CDiff_Hpf[indx] + P_CDiff_Hpf[indx + w1 + 1]);",
    ),
    (
        "float Q_Stat = std::max(epssq, Q_CDiff_Hpf[indx3 + 1] + Q_CDiff_Hpf[indx2] + Q_CDiff_Hpf[indx4]);",
        "float Q_Stat = std::max(epssq, Q_CDiff_Hpf[indx - w1 + 1] + Q_CDiff_Hpf[indx] + Q_CDiff_Hpf[indx + w1 - 1]);",
    ),
]

LEVEL_FREE_RATIO = [
    (
        f"const float {direction}_Est = cfa[indx {step}] * (lpfi + lpfi) / (eps + lpfi + lpf[lpindx {step}]);",
        f"const float {direction}_Est = cfa[indx {step}] * (lpfi + lpfi) / (lpfi + lpf[lpindx {step}]);",
    )
    for direction, step in [("N", "- w1"), ("S", "+ w1"), ("W", "-  1"), ("E", "+  1")]
]

STUBS = {
    "StopWatch.h": "#pragma once\n#define BENCHFUN\nstruct StopWatch { explicit StopWatch(const char*) {} };\n",
    "sleefsseavx.h": "",
}

HARNESS = r"""
#include <cmath>
#include <cstdio>
#include <vector>
#include "librtprocess.h"

// The border is not compared, so librtprocess's border fill is not built.
rpError bayerborder_demosaic(int, int, int, const float *const *, float **, float **, float **, const unsigned[2][2]) { return RP_NO_ERROR; }

constexpr int W = WIDTH_, H = HEIGHT_;

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
    // RGGB, BGGR, GRBG, GBRG: 0 red, 1 green, 2 blue.
    const unsigned patterns[4][2][2] = {{{0, 1}, {1, 2}}, {{2, 1}, {1, 0}}, {{1, 0}, {2, 1}}, {{1, 2}, {0, 1}}};
    for (int scene = 0; scene < 4; ++scene) {
        for (int p = 0; p < 4; ++p) {
            std::vector<float> raw(W * H), r(W * H), g(W * H), b(W * H);
            std::vector<const float *> rows(H);
            std::vector<float *> rr(H), gr(H), br(H);
            for (int y = 0; y < H; ++y) {
                for (int x = 0; x < W; ++x) {
                    // librtprocess divides by 65536, a power of two, so the sample comes back exact.
                    raw[y * W + x] = scene_value(scene, patterns[p][y & 1][x & 1], x, y) * 65536.f;
                }
                rows[y] = &raw[y * W];
                rr[y] = &r[y * W]; gr[y] = &g[y * W]; br[y] = &b[y * W];
            }
            rcd_demosaic(W, H, rows.data(), rr.data(), gr.data(), br.data(), patterns[p], [](double) { return false; }, 2, false, false);
            for (auto *plane : {&r, &g, &b}) {
                for (float &v : *plane) v /= 65536.f;
                std::fwrite(plane->data(), sizeof(float), plane->size(), stdout);
            }
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
        source = (root / "rcd.cc").read_text()
        for old, new in THREE_SITES + LEVEL_FREE_RATIO:
            assert source.count(old) == 1, old
            source = source.replace(old, new)
        (root / "rcd.cc").write_text(source)
        for name, text in STUBS.items():
            (root / name).write_text(text)
        harness = HARNESS.replace("WIDTH_", str(WIDTH)).replace("HEIGHT_", str(HEIGHT))
        (root / "harness.cc").write_text(harness)
        subprocess.run(
            ["g++", "-std=c++17", "-O2", "-ffp-contract=off", "-I", ".", "harness.cc", "rcd.cc", "-o", "harness"],
            cwd=root,
            check=True,
        )
        output = subprocess.run([str(root / "harness")], capture_output=True, check=True).stdout
    samples = array.array("f", output)
    if sys.byteorder != "little":
        samples.byteswap()
    plane = WIDTH * HEIGHT
    for scene in range(4):
        row = []
        for phase in range(4):
            interior = array.array("f")
            for channel in range(3):
                base = ((scene * 4 + phase) * 3 + channel) * plane
                for y in range(BORDER, HEIGHT - BORDER):
                    start = base + y * WIDTH
                    interior.extend(samples[start + BORDER : start + WIDTH - BORDER])
            if sys.byteorder != "little":
                interior.byteswap()
            row.append(f"0x{fnv1a64(interior.tobytes()):016x}")
        print(f"[{', '.join(row)}],")


if __name__ == "__main__":
    main()
