// Accessors for LibRaw state its C API does not expose. Each reaches the `LibRaw` object through
// `libraw_data_t::parent_class`, which the constructor sets, and reads it through the class's own
// public `get_internal_data_pointer`.

#include "libraw/libraw.h"

#include <ctime>

#ifdef _OPENMP
#include <omp.h>
#endif

extern "C" unsigned libraw_lumos_zero_is_bad(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->get_internal_data_pointer()->internal_output_params.zero_is_bad;
}

extern "C" void libraw_lumos_set_decode_threads(int threads)
{
#ifdef _OPENMP
  omp_set_num_threads(threads);
#else
  (void)threads;
#endif
}

extern "C" unsigned libraw_lumos_openmp(void)
{
#ifdef _OPENMP
  return 1;
#else
  return 0;
#endif
}

extern "C" unsigned libraw_lumos_fuji_width(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->is_fuji_rotated();
}

extern "C" unsigned libraw_lumos_is_floating_point(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->is_floating_point() != 0;
}

extern "C" int libraw_lumos_fuji_lossless(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->get_internal_data_pointer()->unpacker_data.fuji_lossless;
}

extern "C" unsigned libraw_lumos_crx_coding(libraw_data_t *lr, int *enc_type, int *image_levels)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  const libraw_internal_data_t *internal = raw->get_internal_data_pointer();
  int track = internal->unpacker_data.crx_track_selected;
  if (track < 0 || track >= LIBRAW_CRXTRACKS_MAXCOUNT || track >= internal->unpacker_data.crx_track_count)
    return 0;
  *enc_type = internal->unpacker_data.crx_header[track].encType;
  *image_levels = internal->unpacker_data.crx_header[track].imageLevels;
  return 1;
}

// LibRaw reads the EXIF capture time with `mktime`, as a local time of the machine decoding it;
// `localtime` in the same process gives the camera's clock back, whatever zone this machine is in.
extern "C" unsigned libraw_lumos_camera_clock(libraw_data_t *lr, char *text, size_t capacity)
{
  time_t stamp = lr->other.timestamp;
  if (stamp <= 0)
    return 0;
  struct tm clock;
#ifdef _WIN32
  if (localtime_s(&clock, &stamp) != 0)
    return 0;
#else
  if (localtime_r(&stamp, &clock) == nullptr)
    return 0;
#endif
  return strftime(text, capacity, "%Y-%m-%dT%H:%M:%S", &clock) != 0;
}
