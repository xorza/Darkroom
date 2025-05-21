<script lang="ts">
    import { Graph, NodeDetails } from '$lib';

    let selected_node_ids: number[] = $state([]);
    const single = $derived(() => (selected_node_ids.length === 1 ? selected_node_ids[0] : null));
    const count = $derived(() => selected_node_ids.length);

    function onSelected(node_ids: number[]) {
        selected_node_ids = node_ids;
    }
</script>

<div class="flex h-screen">
    <div class="flex-1">
        <Graph selectedChange={onSelected}/>
    </div>

    <div class="w-64">
        <NodeDetails nodeId={single()} selectionCount={count()}/>
    </div>
</div>


