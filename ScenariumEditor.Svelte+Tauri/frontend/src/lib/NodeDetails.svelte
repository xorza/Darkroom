<script lang="ts">
    import {invoke} from '@tauri-apps/api/core';
    import type {FuncView, NodeView} from '$lib/types';

    interface NodeDetailsProps {
        // ID of the single selected node. `null` when none or multiple nodes
        // are selected.
        nodeId: string | null;
        // Total number of selected nodes so the view can display state when
        // none or multiple nodes are selected.
        selectionCount?: number;
    }

    let {nodeId = null, selectionCount = 0}: NodeDetailsProps = $props();

    let func: FuncView | null = $state(null);

    async function load() {
        if (nodeId === null) {
            func = null;
            return;
        }
        try {
            const node = await invoke<NodeView>('get_node_by_id', {id: nodeId});
            func = await invoke<FuncView>('get_func_by_id', {id: node.funcId});
        } catch (e) {
            console.error('Failed to load function info', e);
            func = null;
        }
    }

    $effect(() => {
        nodeId;
        load();
    });
</script>

<div class="h-full bg-base-100">
    {#if selectionCount === 0}
        <p class="p-2 text-xs italic">no node selected</p>
    {:else if selectionCount > 1}
        <p class="p-2 text-xs italic">multiple nodes selected</p>
    {:else if func}
        <div class="p-2">
            <h3 class="font-bold mb-1 text-sm">{func.title}</h3>
            <p class="text-xs">{func.description}</p>
        </div>
    {/if}
</div>