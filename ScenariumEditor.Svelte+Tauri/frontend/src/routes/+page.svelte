<script lang="ts">
    import {Graph, NodeDetails} from '$lib';
    import {onMount} from 'svelte';
    import {invoke} from "@tauri-apps/api/core";
    import type {FuncLibraryView} from "$lib/types";

    onMount(async () => {
    });


    let selected_node_ids: string[] = $state([]);
    const single = $derived(() => (selected_node_ids.length === 1 ? selected_node_ids[0] : null));
    const count = $derived(() => selected_node_ids.length);

    function onSelected(node_ids: string[]) {
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


