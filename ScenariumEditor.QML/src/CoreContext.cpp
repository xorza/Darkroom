#include "CoreContext.hpp"

#include "utils/BufferStream.hpp"
#include "utils/interop.hpp"

#include <cassert>


extern "C" {

struct FfiFunc {
    FfiBuf id;
    FfiBuf name;
    FfiBuf category;
    uint32_t behaviour;
    bool output;
    FfiBuf inputs;
    FfiBuf outputs;
    FfiBuf events;
};

struct FfiNode {
    FfiBuf id;
    FfiBuf func_id;
    FfiBuf name;
    bool output;
    bool cache_outputs;
    FfiBuf inputs;
    FfiBuf outputs;
};

__declspec(dllimport) void *create_context();
__declspec(dllimport) void destroy_context(void *ctx);

__declspec(dllimport) FfiBuf get_funcs(void *ctx);
__declspec(dllimport) FfiBuf get_nodes(void *ctx);
__declspec(dllimport) FfiNode new_node(void *ctx, FfiBuf func_id);

}


Ctx::Ctx() {
    this->ctx = create_context();
}

Ctx::~Ctx() {
    destroy_context(this->ctx);
    this->ctx = nullptr;
}

std::vector<Func> Ctx::get_funcs() const {
    Buf buf = Buf{::get_funcs(this->ctx)};

    auto funcs = buf.read_vec<FfiFunc>();
    std::vector<Func> result;
    result.reserve(funcs.size());

    for (uint32_t i = 0; i < funcs.size(); i++) {
        Func func{funcs[i]};
        result.push_back(func);
    }

    return result;
}

std::vector<Node> Ctx::get_nodes() const {
    Buf buf = Buf{::get_nodes(this->ctx)};

    auto nodes = buf.read_vec<FfiNode>();
    std::vector<Node> result;
    result.reserve(nodes.size());

    for (uint32_t i = 0; i < nodes.size(); i++) {
        Node node{nodes[i]};
        result.push_back(node);
    }

    return result;
}

Node Ctx::new_node(const std::string &func_id) const {
    auto buf = Buf{func_id};
    auto ffi_node = ::new_node(this->ctx, buf.ffi_buf);
    return Node{ffi_node};
}

Func::Func(const FfiFunc &ffi_func) {
    this->id = Buf(ffi_func.id).to_string();
    this->name = Buf(ffi_func.name).to_string();
    this->category = Buf(ffi_func.category).to_string();
    this->behaviour = ffi_func.behaviour;
    this->output = ffi_func.output;
    this->inputs = {};
    this->outputs = {};
    this->events = Buf(ffi_func.events).read_vec<std::string>();
}

Node::Node(const FfiNode &ffi_node) {
    this->id = Buf(ffi_node.id).to_string();
    this->func_id = Buf(ffi_node.func_id).to_string();
    this->name = Buf(ffi_node.name).to_string();
    this->output = ffi_node.output;
    this->cache_outputs = ffi_node.cache_outputs;
    this->inputs = {};
    this->outputs = {};
}
