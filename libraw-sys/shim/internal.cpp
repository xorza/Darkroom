// Accessors for LibRaw state its C API does not expose. Each reaches the `LibRaw` object through
// `libraw_data_t::parent_class`, which the constructor sets, and reads it through the class's own
// public `get_internal_data_pointer`.

#include "libraw/libraw.h"

extern "C" unsigned libraw_lumos_zero_is_bad(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->get_internal_data_pointer()->internal_output_params.zero_is_bad;
}

extern "C" unsigned libraw_lumos_fuji_width(libraw_data_t *lr)
{
  LibRaw *raw = static_cast<LibRaw *>(lr->parent_class);
  return raw->get_internal_data_pointer()->internal_output_params.fuji_width;
}
