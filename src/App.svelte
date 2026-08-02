<script lang="ts">
  import Toolbar from "./features/toolbar/Toolbar.svelte";
  import NowPlaying from "./features/deck/NowPlaying.svelte";
  import CueDeck from "./features/deck/CueDeck.svelte";
  import LibraryPanel from "./features/library/LibraryPanel.svelte";
  import PlaylistPanel from "./features/playlist/PlaylistPanel.svelte";
  import SettingsOverlay from "./features/settings/SettingsOverlay.svelte";
  import ScanStatusBar from "./features/scan/ScanStatusBar.svelte";
  import TrackTooltip from "./features/track/TrackTooltip.svelte";
  import EditorOverlay from "./features/track/EditorOverlay.svelte";
  import { app } from "./shared/state.svelte";
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
<EditorOverlay />
