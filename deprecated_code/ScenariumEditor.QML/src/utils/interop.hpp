#pragma once


#include "BufferStream.hpp"

#include <cstdint>
#include <string>
#include <vector>
#include <cassert>

#ifdef _WIN32
#define DLL_IMPORT __declspec(dllimport)
#else
#define DLL_IMPORT
#endif

extern "C" {
struct FfiBuf {
    void *data;
    uint32_t len;
    uint32_t cap;
};
struct FfiUuid {
    uint64_t a;
    uint64_t b;
};
}


struct Buf {
    FfiBuf ffi_buf{};
    bool owns_data = true;

    explicit Buf(FfiBuf ffi_buf) : ffi_buf(ffi_buf), owns_data(false) {}

    explicit Buf(const std::string &str) {
        ffi_buf.len = str.size();
        ffi_buf.cap = str.size();
        ffi_buf.data = malloc(ffi_buf.len);
        memcpy(ffi_buf.data, str.data(), ffi_buf.len);
        owns_data = true;
    }

    ~Buf();

    [[nodiscard]] uint32_t len() const {
        return ffi_buf.len;
    }

    [[nodiscard]] void *data() const {
        return ffi_buf.data;
    }

    [[nodiscard]] std::string to_string() const {
        if (len() == 0) {
            return {};
        }

        return std::string(reinterpret_cast<char *>(data()), len());
    }


    template<class T>
    [[nodiscard]] std::vector<T> read_vec() const {
        if (len() == 0) {
            return {};
        }

        assert(ffi_buf.len % sizeof(T) == 0); // check that the buffer is a multiple of the size of T

        auto len = ffi_buf.len / sizeof(T);
        auto data = static_cast <T *>(ffi_buf.data);

        std::vector<T> result;
        result.reserve(len);
        for (uint32_t i = 0; i < len; i++) {
            result.push_back(data[i]);
        }
        return result;
    }

};

template<>
std::vector<std::string> Buf::read_vec<std::string>() const;

