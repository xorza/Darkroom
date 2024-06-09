#pragma once

#include <vector>
#include <cstdint>
#include <memory>
#include <string>
#include <cstdlib>


struct Func {
    std::string id;
    std::string name;
    std::string category;
    uint32_t behaviour = 0;
    bool output = false;
    std::vector<std::string> inputs;
    std::vector<std::string> outputs;
    std::vector<std::string> events;

};

struct Node {
    std::string id;
    std::string func_id;
    std::string name;
    bool output = false;
    bool cache_outputs = false;
    std::vector<std::string> inputs;
    std::vector<std::string> outputs;
};


struct Ctx {
    void *ctx = nullptr;

    Ctx();

    ~Ctx();

    Ctx(const Ctx &other) = delete;

    Ctx &operator=(const Ctx &other) = delete;

    [[nodiscard]] std::vector<Func> get_funcs() const;
    [[nodiscard]] std::vector<Node> get_nodes() const;
};

