<script lang="ts">
    import {onMount} from 'svelte';
    import {invoke} from '@tauri-apps/api/core';
    import type {FuncLibraryView, FuncView} from "$lib/types";

    interface FuncLibraryProps {
        close: () => void;
        startDrag?: (item: FuncView, event: PointerEvent) => void;
        container: HTMLElement;
        x?: number;
        y?: number;
    }

    let {
        close,
        startDrag,
        container,
        x = 0,
        y = 0,
    }: FuncLibraryProps = $props();

    let func_library: FuncLibraryView = $state({
        funcs: [],
    });
    let search = $state('');

    onMount(async () => {
        try {
            let data = await invoke<FuncLibraryView>('get_func_library');
            func_library = data;
            console.log('Loaded func library', data);
        } catch (e) {
            console.error('Failed to load node library', e);
        }
    });

    const filtered = $derived(() =>
        func_library.funcs.filter(
            (i) =>
                i.title.toLowerCase().includes(search.toLowerCase()) ||
                i.description.toLowerCase().includes(search.toLowerCase())
        )
    );

    let panel: HTMLDivElement;
    let startX = 0;
    let startY = 0;
    let dragging = false;

    let itemDragging: FuncView | null = null;
    let itemPointerId = 0;

    function onPointerDown(event: PointerEvent) {
        if (event.button !== 0) return;
        dragging = true;
        startX = event.clientX - x;
        startY = event.clientY - y;
        panel.setPointerCapture(event.pointerId);
    }

    function onPointerMove(event: PointerEvent) {
        if (!dragging) return;
        let newX = event.clientX - startX;
        let newY = event.clientY - startY;
        const bounds = container.getBoundingClientRect();
        const panelRect = panel.getBoundingClientRect();
        newX = Math.min(Math.max(newX, 0), bounds.width - panelRect.width);
        newY = Math.min(Math.max(newY, 0), bounds.height - panelRect.height);
        x = newX;
        y = newY;
    }

    function onPointerUp(event: PointerEvent) {
        dragging = false;
        panel.releasePointerCapture(event.pointerId);
    }

    function onItemPointerDown(item: FuncView, event: PointerEvent) {
        if (event.button !== 0) return;
        itemDragging = item;
        itemPointerId = event.pointerId;
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    }

    function onItemPointerMove(event: PointerEvent) {
        if (!itemDragging || event.pointerId !== itemPointerId) return;
        const rect = panel.getBoundingClientRect();
        const inside =
            event.clientX >= rect.left &&
            event.clientX <= rect.right &&
            event.clientY >= rect.top &&
            event.clientY <= rect.bottom;
        if (!inside) {
            (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
            startDrag?.(itemDragging, event);
            itemDragging = null;
            close();
        }
    }

    function onItemPointerUp(event: PointerEvent) {
        if (itemDragging && event.pointerId === itemPointerId) {
            (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
            itemDragging = null;
        }
    }
</script>

<button
        class="absolute top-0 left-0 w-full h-full flex items-start justify-start bg-black/30 p-4"
        onclick={close}
        aria-label="Close Func Library"
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
            Functions library
        </h2>
        <input
                type="text"
                class="input input-xs input-bordered w-24 mx-2 p-1"
                placeholder="Search..."
                bind:value={search}
        />

    </div>
    <ul class="flex flex-col gap-0.5 max-h-60 overflow-y-auto pr-1">
        {#each filtered() as item (item.id)}
            <li
                    class="px-1 py-0.5 text-xs border border-base-300 hover:border-primary hover:bg-base-300/50 rounded-sm cursor-pointer transition-colors"
                    title={item.description}
                    onpointerdown={(e) => onItemPointerDown(item, e)}
                    onpointermove={onItemPointerMove}
                    onpointerup={onItemPointerUp}
                    onpointercancel={onItemPointerUp}
            >
                <span class="font-semibold block">{item.title}</span>
            </li>
        {/each}
        {#if filtered().length === 0}
            <li class="text-xs italic p-1">No results</li>
        {/if}
    </ul>
</div>

