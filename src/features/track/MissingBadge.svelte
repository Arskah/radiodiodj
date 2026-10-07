<script lang="ts">
  import { app } from "../../shared/state.svelte";

  let { trackId }: { trackId: number } = $props();

  const since = $derived(app.missingSince.get(trackId));
  const hiddenAt = $derived(app.hiddenAt.get(trackId));
</script>

{#if since !== undefined}
  <span
    class="missing-badge"
    title={`File missing since ${new Date(since).toLocaleString()}`}
    aria-label="File missing"
    ><span class="material-symbols-outlined">link_off</span></span
  >
{:else if hiddenAt !== undefined}
  <span
    class="missing-badge"
    title={`Hidden from the library since ${new Date(hiddenAt).toLocaleString()}`}
    aria-label="Hidden from the library"
    ><span class="material-symbols-outlined">visibility_off</span></span
  >
{/if}
