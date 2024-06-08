#pragma once

#include <cstdint>
#include <type_traits>
#include <stdexcept>
#include <vector>
#include <string>



class BufferStream {
private:
    uint8_t *data = nullptr;
    uint32_t len = 0;
    uint32_t pos = 0;

public:

    ~BufferStream();

    [[nodiscard]] uint32_t get_len() const {
        return len;
    }

    [[nodiscard]] void *get_data() const {
        return data;
    }

    template<typename T>
    [[maybe_unused]] T read() {
        static_assert(std::is_trivially_copyable<T>::value, "T must be trivially copyable");
        if (pos + sizeof(T) > len) {
            throw std::runtime_error("BufferStream: out of bounds");
        }

        T val = *reinterpret_cast<T *>(data + pos);
        pos += sizeof(T);
        return val;
    }


    // partial implementation for vector<T>
    template<typename T>
    [[maybe_unused]] std::vector<T> read_vec() {
        uint32_t vec_len = read<uint32_t>();
        if (pos + vec_len * sizeof(T) > len) {
            throw std::runtime_error("BufferStream: out of bounds");
        }

        std::vector<T> vec;
        vec.reserve(vec_len);
        for (uint32_t i = 0; i < vec_len; i++) {
            vec.push_back(read<T>());
        }
        return vec;
    }

    std::string read_cstr();

    std::string read_str_buf();
};


template<>
[[maybe_unused]] std::string BufferStream::read<std::string>();

