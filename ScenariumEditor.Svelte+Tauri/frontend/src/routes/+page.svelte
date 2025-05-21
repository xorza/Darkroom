<script lang="ts">
    import { Graph, NodeDetails } from '$lib';

    let selected: number[] = $state([]);
    const single = $derived(() => (selected.length === 1 ? selected[0] : null));
    const count = $derived(() => selected.length);

    function onSelected(ids: number[]) {
        selected = ids;
    }
</script>

<div class="flex h-screen">
    <div class="flex-1">
        <Graph selectedChange={onSelected}/>
    </div>

    <div class="w-64">
        <NodeDetails funcId={single()} selectionCount={count()}/>
    </div>
</div>


