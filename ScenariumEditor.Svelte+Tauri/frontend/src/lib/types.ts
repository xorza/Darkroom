interface Pin {
    nodeId: number;
    type: 'input' | 'output';
    index: number;
}

interface NodeView {
    id: number;
    func_id: number;
    x: number;
    y: number;
    title: string;
    inputs: string[];
    outputs: string[];
}

interface ConnectionView {
    fromNodeId: number;
    fromIndex: number;
    toNodeId: number;
    toIndex: number;
}


interface GraphView {
    nodes: NodeView[];
    connections: ConnectionView[];
    viewScale: number;
    viewX: number;
    viewY: number;
    selectedNodeIds: Set<number>
}

interface FuncLibraryItem {
    id: number;
    title: string;
    description: string;
}

export type {
    NodeView,
    ConnectionView,
    GraphView,
    Pin,
    FuncLibraryItem
}
