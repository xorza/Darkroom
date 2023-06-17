#include <string>

#include "graph.hpp"


extern "C" {
void c_graph_init();
void c_graph_deinit();

typedef struct {
    char *name;
    int32_t type;
} C_ArgInfo;

typedef struct {
    unsigned char uuid[16];
    char *name;
    int32_t input_count;
    C_ArgInfo *inputs;
    int32_t output_count;
    C_ArgInfo *outputs;
} C_FunctionInfo;
typedef struct {
    int32_t size;
    C_FunctionInfo *data;
} C_FunctionInfoArray;

C_FunctionInfoArray *c_graph_get_functions();
void c_graph_free_functions(C_FunctionInfoArray const *data);
}

namespace graph_c_api {
    void init() {
        c_graph_init();
    }

    void deinit() {
        c_graph_deinit();
    }

    std::vector<FunctionInfo> get_functions_info() {
        C_FunctionInfoArray const *const funcs = c_graph_get_functions();

        std::vector<FunctionInfo> result{};
        result.reserve(static_cast<size_t>(funcs->size));

        for (int i = 0; i < funcs->size; ++i) {
            C_FunctionInfo const *const c_func = funcs->data + i;
            std::string name(c_func->name);
            uuid_t const *uuid = reinterpret_cast<uuid_t const *>(c_func->uuid);

            FunctionInfo func;
            func.m_name = name;
            uuid_copy(func.m_uuid, *uuid);

            for (int j = 0; j < c_func->input_count; ++j) {
                C_ArgInfo const *const c_arg = c_func->inputs + j;
                std::string arg_name(c_arg->name);

                ArgInfo arg;
                arg.m_name = arg_name;
                arg.m_type = c_arg->type;

                func.m_inputs.emplace_back(arg);
            }

            for (int j = 0; j < c_func->output_count; ++j) {
                C_ArgInfo const *const c_arg = c_func->outputs + j;
                std::string arg_name(c_arg->name);

                ArgInfo arg;
                arg.m_name = arg_name;
                arg.m_type = c_arg->type;

                func.m_outputs.emplace_back(arg);
            }

            result.emplace_back(func);
        }

        c_graph_free_functions(funcs);

        return result;
    }
}