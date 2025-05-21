<script lang="ts">
    import '../app.css';
    import {onMount} from 'svelte';

    onMount(() => {

        const disableContextMenu = (e: Event) => e.preventDefault();
        window.addEventListener('contextmenu', disableContextMenu);

        const disableKeys = (e: KeyboardEvent) => {
            let keys = ['F3', 'F7', 'F11', 'Tab'];
            if (!import.meta.env.DEV) {
                keys = keys.concat(['F5', 'F12']);
            }
            if (keys.includes(e.key)) {
                e.preventDefault();
                e.stopPropagation();
            }
        };
        window.addEventListener('keydown', disableKeys, true);

        return () => {
            window.removeEventListener('contextmenu', disableContextMenu);
            window.removeEventListener('keydown', disableKeys, true);
        };
    });
</script>

<slot/>
