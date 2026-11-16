<script lang="ts">
  import {
    formatStopDuration,
    parseStopDuration,
  } from "../../shared/playlistTiming";

  let {
    plannedDurationSeconds,
    onSave,
    disabled = false,
  }: {
    plannedDurationSeconds: number | null;
    onSave: (seconds: number | null) => void;
    disabled?: boolean;
  } = $props();

  let draft = $state("");
  let input: HTMLInputElement;

  $effect(() => {
    if (document.activeElement !== input) {
      draft = formatStopDuration(plannedDurationSeconds);
    }
  });

  function commit(): void {
    const parsed = parseStopDuration(draft);
    if (parsed === undefined) {
      draft = formatStopDuration(plannedDurationSeconds);
      return;
    }
    draft = formatStopDuration(parsed);
    if (parsed !== plannedDurationSeconds) onSave(parsed);
  }

  function onKeydown(event: KeyboardEvent): void {
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
      input.blur();
    } else if (event.key === "Escape") {
      event.preventDefault();
      draft = formatStopDuration(plannedDurationSeconds);
      input.blur();
    }
  }
</script>

<input
  bind:this={input}
  class="stop-duration-editor"
  type="text"
  inputmode="numeric"
  aria-label="Planned Stop duration (m:ss)"
  title="Planned pause (m:ss); blank leaves it unspecified"
  placeholder="m:ss"
  bind:value={draft}
  {disabled}
  onclick={(event) => event.stopPropagation()}
  onkeydown={onKeydown}
  onblur={commit}
/>

<style>
  .stop-duration-editor {
    width: 4.5em;
    padding: 2px 5px;
    border: 1px solid var(--outline-variant);
    border-radius: var(--r-sm);
    background: var(--surface-container-lowest);
    color: var(--on-surface);
    font-family: var(--font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .stop-duration-editor:focus {
    outline: 1px solid var(--primary);
    border-color: var(--primary);
  }

  .stop-duration-editor:disabled {
    opacity: 0.65;
  }
</style>
