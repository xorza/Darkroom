#include "CoreContext.hpp"

#include "BufferStream.hpp"

#include <cassert>

extern "C" {
struct FfiBuf {
    uint8_t *data;
    uint32_t len;
    uint32_t cap;

};

__declspec(dllimport) void *create_context();
__declspec(dllimport) void destroy_context(void *ctx);
__declspec(dllimport) void destroy_ffi_buf(FfiBuf buf);
__declspec(dllimport) FfiBuf get_funcs(void *ctx);


}

struct Buf {
    FfiBuf ffi_buf;

    explicit Buf(FfiBuf ffi_buf) : ffi_buf(ffi_buf) {}

    ~Buf() {
        destroy_ffi_buf(ffi_buf);
    }

//    Buf(const Buf &other) = delete;
//
//    Buf &operator=(const Buf &other) = delete;

    [[nodiscard]] uint32_t len() const {
        return ffi_buf.len;
    }

    [[nodiscard]] uint8_t *data() const {
        return ffi_buf.data;
    }

    [[nodiscard]] std::string to_string() const {
        return std::string(reinterpret_cast<char *>(data()), len());
    }
};

struct FfiFunc {
    Buf id;
    Buf name;
    Buf category;
    uint32_t behaviour;
    bool output;
    Buf inputs;
    Buf outputs;
    Buf events;
};


Ctx::Ctx() {
    this->ctx = create_context();
}

Ctx::~Ctx() {
    destroy_context(this->ctx);
    this->ctx = nullptr;
}

std::vector<Func> Ctx::get_funcs() const {
    Buf buf = Buf{::get_funcs(this->ctx)};

    assert(buf.len() % sizeof(FfiFunc) == 0); // check that the buffer is a multiple of the size of FfiFunc
    auto len = buf.len() / sizeof(FfiFunc);
    auto funcs = static_cast <FfiFunc *>( static_cast <void *>(buf.data()));

    std::vector<Func> result;
    result.reserve(len);

    for (uint32_t i = 0; i < len; i++) {
        auto ffi_func = funcs[i];
        Func func{
                ffi_func.id.to_string(),
                ffi_func.name.to_string(),
                ffi_func.category.to_string(),
                ffi_func.behaviour,
                ffi_func.output,
                {},
                {},
                {}
//                BufferStream{ffi_func->inputs.data(), ffi_func->inputs.len()}.read_strings(),
//                BufferStream{ffi_func->outputs.data(), ffi_func->outputs.len()}.read_strings(),
//                BufferStream{ffi_func->events.data(), ffi_func->events.len()}.read_strings()
        };
        result.push_back(func);
    }


    return result;
}
