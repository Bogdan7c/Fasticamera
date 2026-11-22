// Заглушка JPEG XL для сборки dng_validate без libjxl: наши DNG не используют JXL.
#include "dng_jxl.h"
#include "dng_exceptions.h"
dng_jxl_decoder::~dng_jxl_decoder () {}
void dng_jxl_decoder::Decode (dng_host &, dng_stream &) { ThrowNotYetImplemented ("JXL"); }
void dng_jxl_decoder::ProcessExifBox (dng_host &, const std::vector<uint8> &) {}
void dng_jxl_decoder::ProcessXMPBox (dng_host &, const std::vector<uint8> &) {}
void dng_jxl_decoder::ProcessBox (dng_host &, const dng_string &, const std::vector<uint8> &) {}
bool ParseJXL (dng_host &, dng_stream &, dng_info &, bool, bool) { return false; }
void PreviewColorSpaceToJXLEncoding (const PreviewColorSpaceEnum, const uint32, dng_jxl_color_space_info &) {}
bool SupportsJXL (const dng_image &) { return false; }
void EncodeJXL_Tile (dng_host &, dng_stream &, const dng_pixel_buffer &, const dng_jxl_color_space_info &, const dng_jxl_encode_settings &) { ThrowNotYetImplemented ("JXL"); }
void EncodeJXL_Container (dng_host &, dng_stream &, const dng_pixel_buffer &, const dng_jxl_encode_settings &, const dng_jxl_color_space_info &, const dng_metadata *, const bool, const bool, const bool, const dng_bmff_box_list *) { ThrowNotYetImplemented ("JXL"); }
