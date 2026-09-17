<script lang="ts">
  import Toolbar from "./features/toolbar/Toolbar.svelte";
  import NowPlaying from "./features/deck/NowPlaying.svelte";
  import CueDeck from "./features/deck/CueDeck.svelte";
  import LibraryPanel from "./features/library/LibraryPanel.svelte";
  import PlaylistPanel from "./features/playlist/PlaylistPanel.svelte";
  import SettingsOverlay from "./features/settings/SettingsOverlay.svelte";
  import ScanStatusBar from "./features/scan/ScanStatusBar.svelte";
  import TrackTooltip from "./features/track/TrackTooltip.svelte";
  import MetadataOverlay from "./features/track/MetadataOverlay.svelte";
  import CuePointOverlay from "./features/track/CuePointOverlay.svelte";
  import UnlockDialog from "./features/admin/UnlockDialog.svelte";
  import { idleLock } from "./features/admin/idleLock";
  import { app } from "./shared/state.svelte";

  const lock = idleLock(
    window,
    () =>
      app.admin.passwordSet && app.admin.unlocked
        ? app.admin.idleLockMin * 60_000
        : null,
    () => app.lockAdmin(),
  );

  $effect(() => {
    void [app.admin.passwordSet, app.admin.unlocked, app.admin.idleLockMin];
    lock.poke();
  });
  $effect(() => lock.dispose);
</script>

<Toolbar />
<div id="workspace" class:no-cue={app.cueDevice === null}>
  <NowPlaying />
  <CueDeck />
  <LibraryPanel />
  <PlaylistPanel />
</div>
<SettingsOverlay />
<ScanStatusBar />
<TrackTooltip />
<MetadataOverlay />
<CuePointOverlay />
<UnlockDialog />
