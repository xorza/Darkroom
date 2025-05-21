
### Use Svelte 5 runes syntax instead of Svelte 3 `$:` syntax


### Svelte createEventDispatcher is deprecated

In Svelte 4, components could emit events by creating a dispatcher with createEventDispatcher.

This function is deprecated in Svelte 5. Instead, components should accept callback props - which means you then pass functions as properties to these components:

```svelte
<script lang="ts">
	import Pump from './Pump.svelte';

	let size = $state(15);
	let burst = $state(false);

	function reset() {
		size = 15;
		burst = false;
	}
</script>

<Pump
-	on:inflate={(power) => {
-		size += power.detail;
+	inflate={(power) => {
+		size += power;
		if (size > 75) burst = true;
	}}
-	on:deflate={(power) => {
-		if (size > 0) size -= power.detail;
+	deflate={(power) => {
+		if (size > 0) size -= power;		
	}}
/>

{#if burst}
	<button onclick={reset}>new balloon</button>
	<span class="boom">💥</span>
{:else}
	<span class="balloon" style="scale: {0.01 * size}">
		🎈
	</span>
{/if}
```

```svelte
<script lang="ts">
-	import { createEventDispatcher } from 'svelte';
-	const dispatch = createEventDispatcher();

+	let { inflate, deflate } = $props();
	let power = $state(5);
</script>

- <button onclick={() => dispatch('inflate', power)}>
+ <button onclick={() => inflate(power)}>
	inflate
</button>
- <button onclick={() => dispatch('deflate', power)}>
+ <button onclick={() => deflate(power)}>
	deflate
</button>
<button onclick={() => power--}>-</button>
Pump power: {power}
<button onclick={() => power++}>+</button>
```