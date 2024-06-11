#include "CoreContext.hpp"

#include "utils/BufferStream.hpp"
#include "utils/interop.hpp"

#include <graph.pb.h>

#include <cassert>

extern "C" {


DLL_IMPORT void *create_context();
DLL_IMPORT void destroy_context(void *ctx);

DLL_IMPORT FfiBuf get_funcs(void *ctx);
DLL_IMPORT FfiBuf get_nodes(void *ctx);
DLL_IMPORT FfiBuf add_node(void *ctx, FfiUuid func_id);
DLL_IMPORT void remove_node(void *ctx, FfiUuid node_id);

}

FfiUuid to_ffi(const uuid &id) {
    return FfiUuid{id.a, id.b};
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
    graph::FuncLib func_lib{};
    func_lib.ParseFromArray(buf.data(), buf.len());

    std::vector<Func> result;
    result.reserve(func_lib.funcs_size());

    for (int i = 0; i < func_lib.funcs_size(); i++) {
        const auto &proto_func = func_lib.funcs(i);

        Func func{};
        func.id = uuid{proto_func.id().a(), proto_func.id().b()};
        func.name = proto_func.name();
        func.category = proto_func.category();
        switch (proto_func.behavior()) {
            case graph::FuncBehavior::ACTIVE:
                func.behaviour = FuncBehavor::Active;
                break;
            case graph::FuncBehavior::PASSIVE:
                func.behaviour = FuncBehavor::Passive;
                break;
            default:
                assert(false);
        }
        func.output = proto_func.is_output();
        func.inputs.reserve(proto_func.inputs_size());
        func.outputs.reserve(proto_func.outputs_size());
        func.events.reserve(proto_func.events_size());

        for (int j = 0; j < proto_func.inputs_size(); j++) {
//            func.inputs.push_back(proto_func.inputs(j));
        }

        for (int j = 0; j < proto_func.outputs_size(); j++) {
//            func.outputs.push_back(proto_func.outputs(j));
        }

        for (int j = 0; j < proto_func.events_size(); j++) {
//            func.events.push_back(proto_func.events(j));
        }

        result.push_back(func);
    }

    return result;
}

std::vector<Node> Ctx::get_nodes() const {
//    Buf buf = Buf{::get_nodes(this->ctx)};
//
//    auto nodes = buf.read_vec<FfiNode>();
//    std::vector<Node> result;
//    result.reserve(nodes.size());
//
//    for (uint32_t i = 0; i < nodes.size(); i++) {
//        Node node{nodes[i]};
//        result.push_back(node);
//    }

    return {};
}

Node Ctx::add_node(const uuid &func_id) const {
//    auto ffi_uuid = to_ffi(func_id);
//    auto ffi_node = ::add_node(this->ctx, ffi_uuid);
    return Node{};
}

void Ctx::remove_node(const uuid &node_id) const {
    auto ffi_uuid = to_ffi(node_id);
    ::remove_node(this->ctx, ffi_uuid);
}
