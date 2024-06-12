#include "CoreContext.hpp"

#include "utils/BufferStream.hpp"
#include "utils/interop.hpp"


#include <json/value.h>
#include <json/json.h>


#include <cassert>
#include <iostream>

extern "C" {


DLL_IMPORT void *create_context();
DLL_IMPORT void destroy_context(void *ctx);

DLL_IMPORT FfiBuf get_func_lib(void *ctx);
DLL_IMPORT FfiBuf get_graph(void *ctx);
DLL_IMPORT void add_node(void *ctx, FfiUuid func_id);
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

Json::Value parse_json(const Buf &buf) {
    Json::Value root;
    Json::CharReaderBuilder readerBuilder;
    std::string errs;

    std::unique_ptr<Json::CharReader> reader(readerBuilder.newCharReader());

    const char *data = static_cast <const char *>(buf.data());
    bool parsingSuccessful = reader->parse(data, data + buf.len(), &root, &errs);

    if (!parsingSuccessful) {
        std::cerr << "Failed to parse JSON: " << errs << std::endl;
        assert(false);
    }

    return root;
}

std::vector<Func> Ctx::get_func_lib() const {
    Buf buf = Buf{::get_func_lib(this->ctx)};
    Json::Value root = parse_json(buf);

    std::vector<Func> funcs{};
    for (const auto &func: root) {
        Func f;
        f.id = uuid::from_string(func["id"].asString());
        f.name = func["name"].asString();
        f.category = func["category"].asString();

        auto behaviorStr = func["behavior"].asString();
        if (behaviorStr == "Active") {
            f.behaviour = FuncBehavor::Active;
        } else if (behaviorStr == "Passive") {
            f.behaviour = FuncBehavor::Passive;
        } else {
            assert(false);
        }

        f.output = func["output"].asBool();


        funcs.push_back(f);
    }

    return funcs;
}

Graph Ctx::get_graph() const {
    Buf buf = Buf{::get_graph(this->ctx)};
    Json::Value root = parse_json(buf);

    auto json_nodes = root["nodes"];
    std::vector<Node> nodes{};

    for (const auto &node: json_nodes) {
        Node n;
        n.id = uuid::from_string(node["id"].asString());
        n.func_id = uuid::from_string(node["func_id"].asString());
        n.name = node["name"].asString();
        n.output = node["output"].asBool();
        n.cache_outputs = node["cache_outputs"].asBool();

        for (const auto &input: node["inputs"]) {
        }

        for (const auto &event: node["events"]) {
        }

        nodes.push_back(n);
    }

    return Graph{nodes};
}

void Ctx::add_node(const uuid &func_id) const {
    auto ffi_uuid = to_ffi(func_id);
    ::add_node(this->ctx, ffi_uuid);
}

void Ctx::remove_node(const uuid &node_id) const {
    auto ffi_uuid = to_ffi(node_id);
    ::remove_node(this->ctx, ffi_uuid);
}
