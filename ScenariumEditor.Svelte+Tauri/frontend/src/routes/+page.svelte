<script lang="ts">
    import { Graph, NodeDetails } from '$lib';

    let selected: number[] = $state([]);
    const single = $derived(() => (selected.length === 1 ? selected[0] : null));

    function onSelected(ids: number[]) {
        selected = ids;
    }
</script>

<div class="flex h-full">
    <div class="flex-1 relative w-96">
        <Graph selectedChange={onSelected}/>
    </div>
    <div class="w-64 border-l border-base-300 overflow-auto">
        <NodeDetails funcId={single()}/>
    </div>
</div>
