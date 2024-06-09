#pragma once

#include <cstdint>
#include <functional>
#include <string>
struct uuid {
    uint64_t a;
    uint64_t b;

    [[nodiscard]] bool operator==(const uuid &other) const {
        return a == other.a && b == other.b;
    }

    [[nodiscard]] bool operator!=(const uuid &other) const {
        return a != other.a || b != other.b;
    }

    [[nodiscard]] bool operator<(const uuid &other) const {
        return a < other.a || (a == other.a && b < other.b);
    }

    [[nodiscard]] bool operator>(const uuid &other) const {
        return a > other.a || (a == other.a && b > other.b);
    }

    [[nodiscard]] bool operator<=(const uuid &other) const {
        return a < other.a || (a == other.a && b <= other.b);
    }

    [[nodiscard]] bool operator>=(const uuid &other) const {
        return a > other.a || (a == other.a && b >= other.b);
    }

    [[nodiscard]] bool is_null() const {
        return a == 0 && b == 0;
    }

    [[nodiscard]] static uuid new_v4();
    [[nodiscard]] static uuid from_string(const std::string &str);
    [[nodiscard]] std::string to_string() const;
};

namespace std {
template<>
struct hash<uuid> {
    std::size_t operator()(const uuid &u) const {
        std::hash<uint64_t> hasher;
        size_t hash = 17;
        hash = hash * 31 + hasher(u.a);
        hash = hash * 31 + hasher(u.b);
        return hash;
    }
};
}
