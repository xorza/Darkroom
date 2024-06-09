#include "uuid.hpp"

#include "interop.hpp"

extern "C" {
__declspec(dllimport) FfiUuid uuid_new_v4_extern();
__declspec(dllimport) FfiUuid uuid_from_string_extern(FfiBuf str);
__declspec(dllimport) FfiBuf uuid_to_string_extern(FfiUuid uuid);
}

uuid uuid::new_v4() {
    FfiUuid ffi = uuid_new_v4_extern();
    return {ffi.a, ffi.b};
}

uuid uuid::from_string(const std::string &str) {
    FfiBuf ffi_str = {
            const_cast<char *>(str.data()),
            static_cast<uint32_t>(str.size()),
            static_cast<uint32_t>(str.capacity())
    };
    FfiUuid ffi = uuid_from_string_extern(ffi_str);
    return {ffi.a, ffi.b};
}

std::string uuid::to_string() const {
    FfiUuid ffi = {a, b};
    auto buf_str = Buf{uuid_to_string_extern(ffi)};;
    return buf_str.to_string();
}
