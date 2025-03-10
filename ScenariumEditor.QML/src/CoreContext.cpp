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

FuncLib Ctx::get_func_lib() const {
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

        for (const auto &input: func["inputs"]) {
            FuncInpu info;
            info.name = input["name"].asString();
            info.is_required = input["is_required"].asBool();

            //todo: parse data_type value
            //todo: parse default_value
            //todo: parse variants

            f.inputs.push_back(info);
        }

        for (const auto &output: func["outputs"]) {
            FuncOutput info;
            info.name = output["name"].asString();

            //todo: parse data_type value

            f.outputs.push_back(info);
        }

        for (const auto &event: func["events"]) {
            FuncEvent info;
            info.name = event["name"].asString();
            f.events.push_back(info);
        }


        funcs.push_back(f);
    }

    return FuncLib{funcs};
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

        for (const auto &json_input: node["inputs"]) {
            NodeInput node_input{};

            const auto &binding = json_input["binding"];
            if (binding.isString()) {
                node_input.output_index = 0;
                node_input.output_node_id = uuid{};

                auto binding_str = binding.asString();
                if (binding_str == "None") {
                    node_input.type = NodeInputType::None;

                } else if (binding_str == "Const") {
                    node_input.type = NodeInputType::Const;
                    //todo: parse const value
                } else {
                    assert(false);
                }
            } else if (binding.isObject()) {
                //todo: parse node output binding
                std::string json_str = binding.toStyledString();

            } else {
                assert(false);
            }

            n.inputs.push_back(node_input);
        }

        for (const auto &event: node["events"]) {
        }

        nodes.push_back(n);
    }

    return Graph{nodes};
}

void Ctx::add_node(const uuid &func_id) const {
    auto ffi_uuid = to_ffi(func_id);
}

void Ctx::remove_node(const uuid &node_id) const {
    auto ffi_uuid = to_ffi(node_id);
}
