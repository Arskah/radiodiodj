<script lang="ts">
  import { api } from "../../shared/api";
  import { app } from "../../shared/state.svelte";
  import { APP_NAME } from "../../shared/appName";
  import type { ImageSlot } from "../../shared/types";

  // Covers "I dropped a folder in, then came here to look for it".
  $effect(() => {
    if (app.settingsOpen) void app.loadThemes();
  });

  // Draft of the station name, resynced whenever the backend hands back a new
  // appearance — it trims and caps, so the input must adopt what was stored.
  let stationName = $state("");
  $effect(() => {
    stationName = app.appearance?.stationName ?? "";
  });

  async function saveStationName(): Promise<void> {
    const next = stationName.trim();
    await app.setStationName(next === "" ? null : next);
  }

  async function pickImage(slot: ImageSlot): Promise<void> {
    const path = await api.pickImageFile();
    if (path) await app.setStationImage(slot, path);
  }

  // The two slots differ in shape, so one image cannot serve both: the toolbar
  // wants wide and short, the record label is cropped to a circle.
  const imageSlots = $derived([
    {
      slot: "logo" as ImageSlot,
      label: "Toolbar logo",
      src: app.appearance?.logo ?? null,
      owned: app.appearance?.logo != null,
      round: false,
      hint: "Replaces the station name in the toolbar. Wide and short suits it best.",
    },
    {
      slot: "label" as ImageSlot,
      label: "Record label",
      src: app.appearance?.label ?? null,
      owned: app.appearance?.label != null,
      round: true,
      hint: "The centre of the deck's vinyl when a track has no cover art. Square, and keep anything important inside the circle — cover art wins when a track has it.",
    },
  ]);

  // Selecting a theme applies it immediately — the running app is the preview,
  // and reverting is picking the previous entry.
  async function pickTheme(themeId: string): Promise<void> {
    await app.setTheme(themeId);
  }

  // Re-enumerates and re-resolves, so editing the active theme and pressing
  // this repaints. A theme that has become invalid leaves the colours alone.
  async function reload(): Promise<void> {
    await app.reloadThemes();
  }
</script>

<div class="settings-section">
  <h4>Theme</h4>
  <p class="settings-section-desc">
    A theme repaints the player. It changes no layout, no wording and no
    behaviour, and the app never repaints itself unasked — drop a folder in,
    then press <em>Reload themes</em>.
  </p>
  {#if app.appearance?.problem}
    <div class="theme-problem" role="status">
      <span class="material-symbols-outlined" aria-hidden="true">warning</span>
      {app.appearance.problem} — the colours on screen are the last good ones.
    </div>
  {/if}
  <div class="theme-list" role="radiogroup" aria-label="Theme">
    {#each app.themes as theme (theme.id + theme.source)}
      {@const active = app.appearance?.themeId === theme.id}
      <button
        id={`appearance-theme-${theme.id}`}
        class="theme-row"
        class:active
        class:invalid={theme.error !== null}
        role="radio"
        aria-checked={active}
        disabled={theme.error !== null}
        onclick={() => pickTheme(theme.id)}
      >
        <span class="material-symbols-outlined theme-mark">
          {theme.error !== null
            ? "block"
            : active
              ? "radio_button_checked"
              : "radio_button_unchecked"}
        </span>
        <span class="theme-text">
          <span class="theme-name">{theme.name}</span>
          <span class="theme-meta">
            {theme.source}{theme.base ? ` · ${theme.base}` : ""}{theme.author
              ? ` · ${theme.author}`
              : ""}
          </span>
          {#if theme.error}
            <span class="theme-error">{theme.error}</span>
          {/if}
        </span>
      </button>
    {/each}
  </div>
  <div class="theme-actions">
    <button id="appearance-reload" class="np-browse" onclick={reload}>
      <span class="material-symbols-outlined" aria-hidden="true">refresh</span>
      Reload themes
    </button>
    <button
      id="appearance-reveal"
      class="np-browse"
      onclick={() => void api.revealThemesDir()}
    >
      <span class="material-symbols-outlined" aria-hidden="true"
        >folder_open</span
      >
      Show in folder
    </button>
  </div>
  <div class="hint">
    Themes live in <code>themes/</code> in the app data folder. Copy the
    <code>example</code> folder, edit the values, then reload.
  </div>
</div>
<div class="settings-section">
  <h4>Station Identity</h4>
  <p class="settings-section-desc">
    Your station's name and artwork. Kept separately from the theme, so they
    survive every theme you try — a theme may ship its own, and yours wins over
    it.
  </p>
  <div class="device-row">
    <label for="appearance-station-name">Station name</label>
    <input
      id="appearance-station-name"
      type="text"
      maxlength="64"
      placeholder={APP_NAME}
      bind:value={stationName}
      onchange={saveStationName}
    />
    <div class="hint">
      Shown in the toolbar and the window title. Leave it empty to use
      {APP_NAME}.
    </div>
  </div>
  {#each imageSlots as slot (slot.slot)}
    <div class="device-row">
      <label for={`appearance-${slot.slot}-choose`}>{slot.label}</label>
      <div class="identity-row">
        <span class="identity-preview" class:round={slot.round}>
          {#if slot.src}
            <img src={slot.src} alt="" />
          {:else}
            <span class="material-symbols-outlined" aria-hidden="true"
              >image</span
            >
          {/if}
        </span>
        <button
          id={`appearance-${slot.slot}-choose`}
          class="np-browse"
          onclick={() => pickImage(slot.slot)}>Choose…</button
        >
        <button
          id={`appearance-${slot.slot}-clear`}
          class="np-browse"
          disabled={!slot.owned}
          onclick={() => void app.clearStationImage(slot.slot)}>Clear</button
        >
      </div>
      <div class="hint">{slot.hint}</div>
    </div>
  {/each}
</div>
