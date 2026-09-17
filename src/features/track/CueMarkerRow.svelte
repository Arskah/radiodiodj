<script lang="ts">
  /**
   * One marker in the cue editor: a time field that takes `m:ss.mmm`,
   * `ss.mmm` or `NNNms`, nudge buttons (Shift ×10, Alt ×100), mark-at-playhead
   * and clear. An unset marker shows what it falls back to. Clicking the row
   * selects the marker for the keyboard.
   */
  import {
    formatCueTime,
    nudgeStep,
    parseCueTime,
  } from "../../shared/cueEditor";
  import type { CueMarkerSpec } from "../../shared/cuePoints";

  interface Props {
    spec: CueMarkerSpec;
    value: number | null;
    /** Shown greyed while unset, e.g. "= Cue In". */
    fallback: string;
    selected: boolean;
    onselect: () => void;
    oncommit: (ms: number | null) => void;
    onnudge: (deltaMs: number) => void;
    onmark: () => void;
  }

  const {
    spec,
    value,
    fallback,
    selected,
    onselect,
    oncommit,
    onnudge,
    onmark,
  }: Props = $props();

  let editing = $state(false);
  let text = $state("");
  let invalid = $state(false);

  const shown = $derived(
    editing ? text : value == null ? "" : formatCueTime(value),
  );

  function onFocus(): void {
    editing = true;
    text = value == null ? "" : formatCueTime(value);
    invalid = false;
    onselect();
  }

  function commit(): void {
    const t = text.trim();
    if (t === "") {
      if (value != null) oncommit(null);
      finish();
      return;
    }
    const ms = parseCueTime(t);
    if (ms == null) {
      invalid = true;
      return;
    }
    if (ms !== value) oncommit(ms);
    finish();
  }

  function finish(): void {
    editing = false;
    invalid = false;
  }

  function onKeyDown(
    e: KeyboardEvent & { currentTarget: HTMLInputElement },
  ): void {
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
      if (!invalid) e.currentTarget.blur();
    } else if (e.key === "Escape") {
      // Revert this field; don't let the dialog take Escape as "close".
      e.preventDefault();
      e.stopPropagation();
      finish();
      e.currentTarget.blur();
    }
  }

  function onBlur(): void {
    if (!editing) return;
    commit();
    if (invalid) finish();
  }
</script>

<div
  class="cue-row"
  class:selected
  role="group"
  aria-label={spec.label}
  onpointerdown={onselect}
>
  <span class="cue-swatch {spec.kind}" aria-hidden="true"></span>
  <label class="cue-row-label" for="cue-field-{spec.key}">
    {spec.label}
    <span class="cue-row-hint">{spec.hint}</span>
  </label>
  <input
    id="cue-field-{spec.key}"
    class="cue-time"
    class:invalid
    class:unset={value == null}
    type="text"
    inputmode="decimal"
    autocomplete="off"
    spellcheck="false"
    placeholder={fallback}
    value={shown}
    aria-invalid={invalid}
    onfocus={onFocus}
    oninput={(e) => {
      text = e.currentTarget.value;
      invalid = false;
    }}
    onkeydown={onKeyDown}
    onblur={onBlur}
  />
  <div class="cue-row-actions">
    <button
      class="btn-mini"
      title="Earlier by 10 ms (Shift 100 ms, Alt 1 s)"
      aria-label="Move {spec.label} earlier"
      onclick={(e) => {
        onselect();
        onnudge(-nudgeStep(e));
      }}
    >
      <span class="material-symbols-outlined" aria-hidden="true"
        >chevron_left</span
      >
    </button>
    <button
      class="btn-mini"
      title="Later by 10 ms (Shift 100 ms, Alt 1 s)"
      aria-label="Move {spec.label} later"
      onclick={(e) => {
        onselect();
        onnudge(nudgeStep(e));
      }}
    >
      <span class="material-symbols-outlined" aria-hidden="true"
        >chevron_right</span
      >
    </button>
    <button
      class="btn-mini"
      data-mark={spec.key}
      title="Set {spec.label} to the playhead"
      aria-label="Set {spec.label} to the playhead"
      onclick={onmark}
    >
      <span class="material-symbols-outlined" aria-hidden="true"
        >my_location</span
      >
    </button>
    <button
      class="btn-mini"
      title="Clear {spec.label}"
      aria-label="Clear {spec.label}"
      disabled={value == null}
      onclick={() => oncommit(null)}
    >
      <span class="material-symbols-outlined" aria-hidden="true">close</span>
    </button>
  </div>
</div>

<style lang="css">
  .cue-row {
    display: grid;
    grid-template-columns: 10px 1fr 120px auto;
    align-items: center;
    gap: 10px;
    padding: 3px 6px;
    border-radius: 4px;
    border: 1px solid transparent;
  }

  .cue-row.selected {
    border-color: color-mix(in srgb, var(--primary) 45%, transparent);
    background: color-mix(in srgb, var(--primary) 6%, transparent);
  }

  .cue-swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }

  .cue-swatch.cue-in {
    background: var(--cue-in-color);
  }
  .cue-swatch.fade-in {
    background: var(--fade-in-color);
  }
  .cue-swatch.fade-out {
    background: var(--fade-out-color);
  }
  .cue-swatch.cue-out {
    background: var(--cue-out-color);
  }
  .cue-swatch.next-start {
    background: var(--next-start-color);
  }

  .cue-row-label {
    display: flex;
    flex-direction: column;
    font-size: 13px;
    color: var(--on-surface);
  }

  .cue-row-hint {
    font-size: 11px;
    color: var(--on-surface-variant);
  }

  .cue-time {
    background: transparent;
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 40%, transparent);
    border-radius: 4px;
    padding: 5px 8px;
    color: var(--on-surface);
    font-family: var(--font-mono);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    outline: none;
  }

  .cue-time::placeholder {
    color: var(--on-surface-variant);
    opacity: 0.6;
  }

  .cue-time:focus {
    border-color: var(--primary);
  }

  .cue-time.invalid {
    border-color: var(--error);
  }

  .cue-row-actions {
    display: flex;
    gap: 4px;
  }

  .btn-mini {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 26px;
    padding: 0;
    background: transparent;
    border: 1px solid
      color-mix(in srgb, var(--outline-variant) 40%, transparent);
    border-radius: 4px;
    color: var(--on-surface-variant);
    cursor: pointer;
  }

  .btn-mini .material-symbols-outlined {
    font-size: 16px;
  }

  .btn-mini:hover:not(:disabled) {
    background: color-mix(in srgb, var(--on-surface) 8%, transparent);
    color: var(--on-surface);
  }

  .btn-mini:disabled {
    opacity: 0.35;
    cursor: default;
  }
</style>
