#!/usr/bin/env bash
#
# clone-refs.sh — shallow-clone the upstream software whose functionality
# overlaps lumos, into .tmp/refs/ for source investigation (Read/Grep without
# per-file registry prompts). Reference only; nothing here is built or linked.
#
# Each entry notes the lumos module(s) it informs. Native deps are pinned to the
# versions in the workspace Cargo.lock; everything else tracks upstream HEAD.
#
# Not cloneable (closed source or no git host) — listed for the record only:
#   PixInsight (proprietary), ASTAP (SourceForge, non-git), AstroPixelProcessor.
#
# Usage:
#   scripts/clone-refs.sh          # core set (focused, mostly small/medium)
#   scripts/clone-refs.sh --all    # core + large/broad suites (GBs)
#   scripts/clone-refs.sh --list   # print what would be cloned, clone nothing
#
# Idempotent: an existing clone is left in place (delete its dir to refresh).

set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/.tmp/refs"
mkdir -p "$DEST"

# name | git url | ref (blank = default branch) | lumos module it informs
CORE=(
  # --- actual native dependencies, pinned to Cargo.lock ---
  "LibRaw|https://github.com/LibRaw/LibRaw|0.20.1|raw: unpack, adjust_bl() black levels, white balance"
  "cfitsio|https://github.com/HEASARC/cfitsio||astro_image/fits: FITS I/O, header parsing"
  "rust-fitsio|https://github.com/simonrw/rust-fitsio||astro_image/fits: the fitsio Rust binding lumos uses"

  # --- calibration ---
  "ccdproc|https://github.com/astropy/ccdproc||calibration_masters: bias/dark/flat reduction, combine"

  # --- demosaic ---
  "librtprocess|https://github.com/CarVac/librtprocess||raw/demosaic: RCD (Ratio-Corrected) + X-Trans Markesteijn origin"

  # --- star detection / photometry / PSF ---
  "sep|https://github.com/kbarbary/sep||star_detection: SExtractor C lib — background, threshold, multi-threshold deblend"
  "sextractor|https://github.com/astromatic/sextractor||star_detection: original SExtractor (detection/deblend/background)"
  "photutils|https://github.com/astropy/photutils||star_detection: DAOFIND roundness (GROUND/SROUND), sharpness, centroiding"
  "psfex|https://github.com/astromatic/psfex||star_detection/centroid: PSF modeling (Gaussian/Moffat fits)"

  # --- registration / alignment / distortion / warp ---
  "astroalign|https://github.com/quatrope/astroalign||registration/triangle: invariant-triangle star matching"
  "magsac|https://github.com/danini/magsac||registration/ransac: MAGSAC++ continuous-loss estimator"
  "astrometry.net|https://github.com/dstndstn/astrometry.net||registration: geometric hashing, plate solving, TAN-SIP"
  "scamp|https://github.com/astromatic/scamp||registration/distortion: astrometric solution, PV/SIP distortion fit"
  "swarp|https://github.com/astromatic/swarp||registration/warp + stacking: resampling, warp, coaddition"
  "reproject|https://github.com/astropy/reproject||registration/warp + drizzle: image reprojection/resampling/coadd"
  "stellarsolver|https://github.com/rlancaste/stellarsolver||detection+registration: SEP+astrometry.net solver library"

  # --- drizzle ---
  "drizzle|https://github.com/spacetelescope/drizzle||drizzle: Fruchter & Hook variable-pixel reconstruction (cdrizzle)"

  # --- broad pipeline reference ---
  "siril|https://gitlab.com/free-astro/siril||all: full astro calibration/registration/stacking/drizzle app"
)

EXTRA=(
  "RawTherapee|https://github.com/Beep6581/RawTherapee||raw/demosaic: RCD/AMaZE/Markesteijn upstream (large)"
  "opencv|https://github.com/opencv/opencv||registration: findHomography, DLT, RANSAC reference (large)"
  "astropy|https://github.com/astropy/astropy||astro_image + registration: FITS, WCS, SIP polynomials (large)"
  "drizzlepac|https://github.com/spacetelescope/drizzlepac||drizzle: full DrizzlePac pipeline"
  "DeepSkyStacker|https://github.com/deepskystacker/DSS||stacking/registration: alternate stacking pipeline"
  "kstars|https://github.com/KDE/kstars||all: KStars/Ekos capture+processing suite (large)"
  "GraXpert|https://github.com/Steffenhir/GraXpert||star_detection/background: gradient/background extraction"
  "sirilic|https://gitlab.com/free-astro/sirilic||stacking: Siril batch-processing frontend"
)

clone_one() {
  local name url ref note dir
  IFS='|' read -r name url ref note <<<"$1"
  dir="$DEST/$name"
  if [ -d "$dir/.git" ]; then
    printf '  [skip] %-16s (already cloned) — %s\n' "$name" "$note"
    return 0
  fi
  printf '  [pull] %-16s %s%s\n' "$name" "$url" "${ref:+ @ $ref}"
  if [ -n "$ref" ]; then
    git clone --depth 1 --branch "$ref" "$url" "$dir" 2>&1 | sed 's/^/         /'
  else
    git clone --depth 1 "$url" "$dir" 2>&1 | sed 's/^/         /'
  fi
}

list_one() {
  local name url ref note
  IFS='|' read -r name url ref note <<<"$1"
  printf '  %-16s %-50s %s\n' "$name" "$url${ref:+ @ $ref}" "$note"
}

MODE="core"
case "${1:-}" in
  --all)  MODE="all" ;;
  --list) MODE="list" ;;
  "")     ;;
  *) echo "unknown option: $1" >&2; exit 2 ;;
esac

if [ "$MODE" = "list" ]; then
  echo "CORE:";  for e in "${CORE[@]}";  do list_one "$e"; done
  echo "EXTRA (--all):"; for e in "${EXTRA[@]}"; do list_one "$e"; done
  exit 0
fi

echo "Cloning reference sources into $DEST"
fail=0
for e in "${CORE[@]}"; do clone_one "$e" || fail=$((fail+1)); done
if [ "$MODE" = "all" ]; then
  for e in "${EXTRA[@]}"; do clone_one "$e" || fail=$((fail+1)); done
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "Done. All requested clones present."
else
  echo "Done with $fail failure(s) — re-run to retry (existing clones are skipped)."
fi
echo "Refresh a clone by deleting .tmp/refs/<name> and re-running."
