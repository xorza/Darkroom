<script lang="ts">
    import { invoke } from '@tauri-apps/api/core';
    import type { FuncView } from '$lib/types';

    interface NodeDetailsProps {
        funcId: number | null;
    }

    let { funcId = null }: NodeDetailsProps = $props();

    let func: FuncView | null = $state(null);

    $effect(async () => {
        funcId;
        if (funcId === null) {
            func = null;
            return;
        }
        try {
            func = await invoke<FuncView | null>('get_func_by_id', { id: funcId });
        } catch (e) {
            console.error('Failed to load function info', e);
            func = null;
        }
    });
</script>

{#if func}
<div class="p-2">
    <h3 class="font-bold mb-1 text-sm">{func.title}</h3>
    <p class="text-xs">{func.description}</p>
</div>
{/if}
