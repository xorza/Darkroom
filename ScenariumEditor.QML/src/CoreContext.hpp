#pragma once

#include "utils/uuid.hpp"

#include <vector>
#include <cstdint>
#include <memory>
#include <string>
#include <cstdlib>

struct Func {
    uuid id;
    std::string name;
    std::string category;
    uint32_t behaviour = 0;
    bool output = false;
    std::vector<std::string> inputs;
    std::vector<std::string> outputs;
    std::vector<std::string> events;

    Func() = default;
};

struct Node {
    uuid id;
    uuid func_id;
    std::string name;
    bool output = false;
    bool cache_outputs = false;
    std::vector<std::string> inputs;
    std::vector<uuid> events;

    Node() = default;
};


struct Ctx {
    void *ctx = nullptr;

    Ctx();

    ~Ctx();

    Ctx(const Ctx &other) = delete;

    Ctx &operator=(const Ctx &other) = delete;

    [[nodiscard]] std::vector<Func> get_funcs() const;

    [[nodiscard]] std::vector<Node> get_nodes() const;

    [[nodiscard]] Node add_node(const uuid &func_id) const;

    void remove_node(const uuid &node_id) const;
};

