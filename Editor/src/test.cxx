export module test_module;

extern "C" {
typedef void *GraphPtr;
GraphPtr graph_new();
void graph_free(GraphPtr graph);
}

export void test() {
    GraphPtr graph = graph_new();
    graph_free(graph);
}