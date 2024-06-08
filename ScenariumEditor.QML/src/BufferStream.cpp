#include "BufferStream.hpp"




BufferStream::~BufferStream() {
    delete[] data;
}


template<>
[[maybe_unused]] std::string BufferStream::read<std::string>() {
    uint32_t str_len = read<uint32_t>();
    if (pos + str_len > len) {
        throw std::runtime_error("BufferStream: out of bounds");
    }

    std::string str(reinterpret_cast<char *>(data + pos), str_len);
    pos += str_len;
    return str;
}

std::string BufferStream::read_cstr() {
    std::string str;
    while (pos < len) {
        char c = *reinterpret_cast<char *>(data + pos);
        pos += 1;
        if (c == '\0') {
            break;
        }
        str.push_back(c);
    }
    return str;
}
