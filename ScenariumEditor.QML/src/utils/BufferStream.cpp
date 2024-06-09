#include "BufferStream.hpp"




BufferStream::~BufferStream() {
}


template<>
[[maybe_unused]] std::string BufferStream::read<std::string>() {
    uint32_t str_len = read<uint32_t>();
    if (_pos + str_len > _len) {
        throw std::runtime_error("BufferStream: out of bounds");
    }

    std::string str(reinterpret_cast<char *>(_data + _pos), str_len);
    _pos += str_len;
    return str;
}
