<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let { trackId }: { trackId: number } = $props();

  const since = $derived(app.missingSince.get(trackId));
</script>

{#if since !== undefined}
  <span
    class="missing-badge"
    title={`File missing since ${new Date(since).toLocaleString()}`}
    aria-label="File missing"
    ><span class="material-symbols-outlined">link_off</span></span
  >
{/if}
