#include "interop.hpp"


extern "C" {
__declspec(dllimport) void destroy_ffi_buf(FfiBuf buf);
}


template<>
std::vector<std::string> Buf::read_vec<std::string>() const {
    if (len() == 0) {
        return {};
    }

    BufferStream stream{data(), len()};
    auto len = stream.read<uint32_t>();

    std::vector<std::string> result{};
    result.reserve(len);
    for (uint32_t i = 0; i < len; i++) {
        result.push_back(stream.read<std::string>());
    }

    return result;
}


Buf::~Buf() {
    if (owns_data) {
        if (ffi_buf.data != nullptr) {
            free(ffi_buf.data);
        }
        ffi_buf.data = nullptr;
        ffi_buf.len = 0;
        ffi_buf.cap = 0;
    } else {
        destroy_ffi_buf(ffi_buf);
    }
}