#pragma once

#include <vector>
#include <uuid/uuid.h>

namespace graph_c_api {
    struct ArgInfo {
        std::string m_name;
        int32_t m_type;
    };

    struct FunctionInfo {
        std::string m_name;
        uuid_t m_uuid;
        std::vector<ArgInfo> m_inputs;
        std::vector<ArgInfo> m_outputs;
    };

    void init();

    void deinit();

    std::vector<FunctionInfo> get_functions_info();
}