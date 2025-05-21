<script lang="ts">
    import { onMount } from 'svelte';
    import { invoke } from '@tauri-apps/api/core';

    interface NodeLibraryProps {
        close: () => void;
        x?: number;
        y?: number;
    }

    interface NodeLibraryItem {
        id: number;
        title: string;
        description: string;
    }

    let {
        close,
        x = 0,
        y = 0,
    }: NodeLibraryProps = $props();

    let items: NodeLibraryItem[] = $state([])
    let search = $state('');

    onMount(async () => {
        try {
            items = await invoke<NodeLibraryItem[]>('get_node_library');
            console.log('Node library items:', items);
        } catch (e) {
            console.error('Failed to load node library', e);
        }
    });

    const filtered = $derived(() =>
        items.filter(
            (i) =>
                i.title.toLowerCase().includes(search.toLowerCase()) ||
                i.description.toLowerCase().includes(search.toLowerCase())
        )
    );

    let panel: HTMLDivElement;
    let startX = 0;
    let startY = 0;
    let dragging = false;

    function onPointerDown(event: PointerEvent) {
        if (event.button !== 0) return;
        dragging = true;
        startX = event.clientX - x;
        startY = event.clientY - y;
        panel.setPointerCapture(event.pointerId);
    }

    function onPointerMove(event: PointerEvent) {
        if (!dragging) return;
        x = event.clientX - startX;
        y = event.clientY - startY;
    }

    function onPointerUp(event: PointerEvent) {
        dragging = false;
        panel.releasePointerCapture(event.pointerId);
    }
</script>

<button
        class="absolute top-0 left-0 w-full h-full flex items-start justify-start bg-black/30 p-4"
        onclick={close}
        aria-label="Close Node Library"
>
</button>

<div
        class=" absolute bg-base-200 border border-base-300 rounded-md shadow-lg p-2 m-2"
        style="transform: translate({x}px, {y}px);"
>
    <div class="flex items-center mb-2">
        <h2
                bind:this={panel}
                class="font-bold text-sm"
                onpointerdown={onPointerDown}
                onpointermove={onPointerMove}
                onpointerup={onPointerUp}
                onpointercancel={onPointerUp}
        >
            Nodes
        </h2>
        <input
                type="text"
                class="input input-xs input-bordered w-24 mx-2 p-1"
                placeholder="Search..."
                bind:value={search}
        />

    </div>
    <ul class="flex flex-col gap-1 max-h-60 overflow-y-auto pr-1">
        {#each filtered() as item (item.id)}
            <li class="p-1 border-b border-base-300 last:border-0">
                <span class="font-semibold text-sm block">{item.title}</span>
                <span class="text-xs opacity-75">{item.description}</span>
            </li>
        {/each}
        {#if filtered().length === 0}
            <li class="text-xs italic p-1">No results</li>
        {/if}
    </ul>
</div>

