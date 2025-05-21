#pragma once

#include "utils/uuid.hpp"

#include <vector>
#include <cstdint>
#include <memory>
#include <string>
#include <cstdlib>
#include <optional>

enum class FuncBehavior {
    Active = 1,
    Passive = 2,
};

struct DataType {

};
struct StaticValue {

};

struct FuncInput {
    std::string name{};
    bool is_required = false;
    DataType data_type{};
    std::optional<StaticValue> default_value{};
    std::vector<StaticValue> variants{};
};
struct FuncOutput {
    std::string name{};
    DataType data_type{};
};
struct FuncEvent {
    std::string name{};
};

struct Func {
    uuid id{};
    std::string name{};
    std::string category{};
    FuncBehavior behavior{};
    bool output = false;
    std::vector<FuncInput> inputs{};
    std::vector<FuncOutput> outputs{};
    std::vector<FuncEvent> events{};

    Func() = default;
};

struct FuncLib {
    std::vector<Func> funcs{};
};

struct NodeEvent {
    std::vector<uuid> subscribers{};
};

enum class NodeInputType {
    None = 1,
    Output = 2,
    Const = 3,
};

struct NodeInput {
    NodeInputType type{};
    uuid output_node_id{};
    uint32_t output_index{};
    std::optional<StaticValue> const_value{};
};

struct Node {
    uuid id{};
    uuid func_id{};
    std::string name{};
    bool output = false;
    bool cache_outputs = false;
    std::vector<NodeInput> inputs{};
    std::vector<NodeEvent> events{};

    Node() = default;
};

struct Graph {
    std::vector<Node> nodes{};
};


struct Ctx {
    void *ctx = nullptr;

    Ctx();

    ~Ctx();

    Ctx(const Ctx &other) = delete;

    Ctx &operator=(const Ctx &other) = delete;

    [[nodiscard]] FuncLib get_func_lib() const;

    [[nodiscard]] Graph get_graph() const;

    void add_node(const uuid &func_id) const;

    void remove_node(const uuid &node_id) const;
};

