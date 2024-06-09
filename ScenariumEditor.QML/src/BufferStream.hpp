#pragma once

#include <cstdint>
#include <type_traits>
#include <stdexcept>
#include <vector>
#include <string>


class BufferStream {
private:
    uint8_t *_data = nullptr;
    uint32_t _len = 0;
    uint32_t _pos = 0;

public:
    BufferStream(void *data, uint32_t len) : _data(static_cast<uint8_t *>(data)), _len(len) {}

    ~BufferStream();

    [[nodiscard]] uint32_t len() const {
        return _len;
    }

    [[nodiscard]] void *data() const {
        return _data;
    }

    template<typename T>
    [[maybe_unused]] T read() {
        static_assert(std::is_trivially_copyable<T>::value, "T must be trivially copyable");
        if (_pos + sizeof(T) > _len) {
            throw std::runtime_error("BufferStream: out of bounds");
        }

        T val = *reinterpret_cast<T *>(_data + _pos);
        _pos += sizeof(T);
        return val;
    }


    // partial implementation for vector<T>
    template<typename T>
    [[maybe_unused]] std::vector<T> read_vec() {
        uint32_t vec_len = read<uint32_t>();
        if (_pos + vec_len * sizeof(T) > _len) {
            throw std::runtime_error("BufferStream: out of bounds");
        }

        std::vector<T> vec;
        vec.reserve(vec_len);
        for (uint32_t i = 0; i < vec_len; i++) {
            vec.push_back(read<T>());
        }
        return vec;
    }
};


template<>
[[maybe_unused]] std::string BufferStream::read<std::string>();

