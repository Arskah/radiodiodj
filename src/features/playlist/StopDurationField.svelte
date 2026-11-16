<script lang="ts">
  import { formatTime } from "../../shared/state.svelte";
  import { parsePlannedDuration } from "../../shared/savedPlaylists";

  interface Props {
    /** The plan in seconds; `null` is unplanned. */
    value: number | null;
    /** Read-only shows the plan as text, or nothing without one. */
    readonly?: boolean;
    oncommit: (seconds: number | null) => void;
  }

  let { value, readonly = false, oncommit }: Props = $props();

  const shown = $derived(value === null ? "" : formatTime(value));
  let text = $state("");
  let refused = $state(false);

  $effect(() => {
    text = shown;
  });

  function commit(): void {
    const seconds = parsePlannedDuration(text);
    if (seconds === undefined) {
      refused = true;
      text = shown;
      return;
    }
    refused = false;
    if (seconds !== value) oncommit(seconds);
  }

  function onKeyDown(e: KeyboardEvent): void {
    // A row's own keys — pick, menu — are not this field's.
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      (e.currentTarget as HTMLInputElement).blur();
    } else if (e.key === "Escape") {
      text = shown;
      refused = false;
      (e.currentTarget as HTMLInputElement).blur();
    }
  }
</script>

{#if readonly}
  <span class="stop-duration">{shown}</span>
{:else}
  <input
    class="stop-duration"
    class:refused
    type="text"
    inputmode="numeric"
    placeholder="m:ss"
    title="Planned length of the break, as m:ss. Blank for none."
    aria-label="Planned duration"
    aria-invalid={refused}
    bind:value={text}
    oninput={() => (refused = false)}
    onchange={commit}
    onkeydown={onKeyDown}
    onclick={(e) => e.stopPropagation()}
    ondblclick={(e) => e.stopPropagation()}
    draggable="false"
  />
{/if}
