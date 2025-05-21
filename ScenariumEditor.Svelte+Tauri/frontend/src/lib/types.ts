interface Pin {
    nodeId: string;
    type: 'input' | 'output';
    index: number;
}

interface NodeView {
    id: string;
    funcId: string;
    viewPosX: number;
    viewPosY: number;
    title: string;
    inputs: string[];
    outputs: string[];
}

interface ConnectionView {
    fromNodeId: string;
    fromIndex: number;
    toNodeId: string;
    toIndex: number;
}


interface GraphView {
    nodes: NodeView[];
    connections: ConnectionView[];
    viewScale: number;
    viewPosX: number;
    viewPosY: number;
    selectedNodeIds: Set<string>
}

interface FuncView {
    id: string;
    title: string;
    description: string;
}

interface FuncLibraryView {
    funcs: FuncView[];
}

export type {
    NodeView,
    ConnectionView,
    GraphView,
    Pin,
    FuncView,
    FuncLibraryView
}
