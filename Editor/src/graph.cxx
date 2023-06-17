export module graph;

extern "C" {
typedef void *GraphPtr;
GraphPtr graph_new();
void graph_free(GraphPtr graph);
}

export class Graph {
    GraphPtr m_ptr = nullptr;

public:
    Graph() : m_ptr(graph_new()) {}

    ~Graph() { graph_free(m_ptr); }
};
