<script lang="ts">
    import type {NodeView} from "$lib/types";
    import type {Pin} from "$lib/types";

    interface NodeProps {
        nodeView: NodeView;

        viewScale: number;
        viewX: number;
        viewY: number;

        selected: boolean;

        registerPin?: (detail: Pin & { el: HTMLElement }) => void;
        connectionStart?: (detail: Pin & { x: number; y: number }) => void;
        connectionEnd?: (pin: Pin) => void;
        drag?: (detail: { nodeId: string; dx: number; dy: number }) => void;
        dragEnd?: (id: string) => void;
        select?: (detail: { nodeId: string; shiftKey: boolean }) => void;
        remove?: (id: string) => void;
    }

    let {
        nodeView,

        viewScale = 1,
        viewX = 0,
        viewY = 0,

        selected = false,

        registerPin,
        connectionStart,
        connectionEnd,
        drag,
        dragEnd,
        select,
        remove,
    }: NodeProps = $props();

    let panel: HTMLDivElement;
    let offsetX = 0;
    let offsetY = 0;
    let dragging = false;

    function onPointerDown(event: PointerEvent) {
        dragging = true;
        offsetX = (event.clientX - viewX) / viewScale - nodeView.x;
        offsetY = (event.clientY - viewY) / viewScale - nodeView.y;
        panel.setPointerCapture(event.pointerId);
    }

    function onPointerMove(event: PointerEvent) {
        if (dragging) {
            const newX = (event.clientX - viewX) / viewScale - offsetX;
            const newY = (event.clientY - viewY) / viewScale - offsetY;
            const dx = newX - nodeView.x;
            const dy = newY - nodeView.y;
            drag?.({nodeId: nodeView.id, dx, dy});
        }
    }

    function onPointerUp(event: PointerEvent) {
        if (dragging) {
            dragging = false;
            dragEnd?.(nodeView.id);
        }
        panel.releasePointerCapture(event.pointerId);
    }

    function registerPinEl(el: HTMLElement | null, type: 'input' | 'output', index: number) {
        if (el && registerPin) {
            registerPin({nodeId: nodeView.id, type, index, el});
        }
    }

    function pinAction(el: HTMLElement, params: { type: 'input' | 'output'; index: number }) {
        registerPinEl(el, params.type, params.index);
        return {
            update(newParams: { type: 'input' | 'output'; index: number }) {
                registerPinEl(el, newParams.type, newParams.index);
            }
        };
    }

    function onPinDown(event: PointerEvent, type: 'input' | 'output', index: number) {
        if (event.button !== 0) return;
        connectionStart?.({
            nodeId: nodeView.id,
            type,
            index,
            x: event.clientX,
            y: event.clientY
        });
    }

    function onPinUp(event: PointerEvent, type: 'input' | 'output', index: number) {
        if (event.button !== 0) return;
        connectionEnd?.({nodeId: nodeView.id, type, index});
    }

    function onNodePointerDown(event: PointerEvent) {
        if (event.button !== 0) return;

        select?.({nodeId: nodeView.id, shiftKey: event.shiftKey});
    }
</script>


<div
        class="absolute border bg-base-300 rounded-md shadow-md select-none pt-0 pb-2 transition-shadow hover:shadow-lg"
        class:border-primary={selected}
        class:border-base-300={!selected}
        style="transform: translate({nodeView.x}px, {nodeView.y}px);"
        data-node-id={nodeView.id}
        onpointerdown={onNodePointerDown}
>
    <h3
            class="font-bold text-center text-sm pb-1 pt-1 px-6 relative"
            bind:this={panel}
            onpointerdown={onPointerDown}
            onpointermove={onPointerMove}
            onpointerup={onPointerUp}
            onpointercancel={onPointerUp}
    >

        {nodeView.title}

        <button
                class="btn btn-ghost btn-xs btn-circle absolute right-0 top-0 m-0 p-0 text-error"
                style="font-size: 0.5rem; height: 1.15rem; width: 1.15rem;"
                onclick={() => remove?.(nodeView.id)}
                onpointerdown={(e) => e.stopPropagation()}
                onpointerup={(e) => e.stopPropagation()}
        >✕</button>
    </h3>
    <div class="flex flex-row gap-1">
        <div
                id="inputs"
                class="flex flex-col items-start justify-start"
                style="transform: translate(-0.25rem, 0);"
        >
            {#each nodeView.inputs as input, i}
                <div class="relative pl-3 pr-2 text-xs">
                    <span
                            class="absolute left-0 top-1/2 w-2.5 h-2.5 -translate-y-1/2 rounded-full bg-primary hover:bg-blue-500 transition-colors"
                            use:pinAction={{ type: 'input', index: i }}
                            onpointerdown={(e) => onPinDown(e, 'input', i)}
                            onpointerup={(e) => onPinUp(e, 'input', i)}
                    ></span>
                    {input}
                </div>
            {/each}
        </div>
        <div
                id="outputs"
                class="flex flex-col items-end justify-start ml-auto"
                style="transform: translate(0.25rem, 0);"
        >
            {#each nodeView.outputs as output, i}
                <div class="relative pl-2 pr-3 text-xs output">
                    {output}
                    <span
                            class="absolute right-0 top-1/2 w-2.5 h-2.5 -translate-y-1/2 rounded-full bg-primary hover:bg-blue-500 transition-colors"
                            use:pinAction={{ type: 'output', index: i }}
                            onpointerdown={(e) => onPinDown(e, 'output', i)}
                            onpointerup={(e) => onPinUp(e, 'output', i)}
                    ></span>
                </div>
            {/each}
        </div>
    </div>
</div>
