# Updates

The app tells an operator when a newer version exists and, when an admin says
so, installs it and restarts. _Settings → About_ shows the running version and
is where both happen.

## Checking is automatic, installing is a click

This is playout software. A restart is a few seconds of dead air, and nobody at
the station chose the moment a release went out. So:

- The app **checks** shortly after launch and every six hours. A check reads one
  small JSON file and changes nothing.
- It never **downloads** or **installs** on its own. Both happen when an admin
  presses _Download and restart_, and with something on air the button asks
  again before it goes.
- It does not refuse an install while something is on air either. The operator
  at the desk decides; the app's part is to make sure they know what the press
  costs.

A check nobody asked for never reports a failure. A station whose uplink is
down for the night has not got an update problem, so the error is logged and
the state goes back to what it was — keeping an offer an earlier check found. A
check started from the button does report.

_Settings → About → Check automatically_ turns the timer off, for a station
that must not reach the internet unprompted. It is read each round, so it takes
effect without a restart.

## Where it looks

`https://radiodiodj.org/update.json` — built by the website from the latest
GitHub release, not served by GitHub. Why, and how the file is put together, is
[website.md](./website.md#updates). How the bundles come to carry signatures is
[signing.md](./signing.md#updater-signing).

An update is only installed if its signature verifies against the public key
compiled into the running app (`plugins.updater.pubkey` in `tauri.conf.json`).
The manifest itself is not signed; it is fetched over TLS, and all it can do is
point at a bundle that must still verify.

## What can update itself

| installed as            | updates itself | how                                               |
| ----------------------- | -------------- | ------------------------------------------------- |
| macOS `.app` (from dmg) | yes            | the `.app` is replaced, then the app restarts     |
| Windows NSIS (`.exe`)   | yes            | the installer runs in passive mode and relaunches |
| Windows MSI             | yes            | the same, with the MSI                            |
| Linux AppImage          | yes            | the AppImage file is replaced, then a restart     |
| Linux `.deb` / `.rpm`   | no             | offered, with a link to the website               |
| unbundled (`pnpm dev`)  | no             | —                                                 |

A `.deb` or `.rpm` belongs to the system's package manager, and replacing it
means a root prompt on a machine that may have nobody at it who can answer
one. Those installs are told a newer version exists and sent to radiodiodj.org.

The manifest's keys name the installer type, so an install is only ever offered
its own kind: an MSI install is never handed the NSIS installer.

## State

The backend owns the updater (`src-tauri/src/update.rs`); the renderer mirrors
one whole snapshot from `update:state`, the way it mirrors the playlist.

```text
idle ─▶ checking ─▶ upToDate
                 ├▶ available ─▶ downloading ─▶ installing ─▶ (restart)
                 └▶ failed                └──────────┴▶ failed
```

`offer` is separate from `phase` and outlives it: once a check has found a
newer release, a later failed check does not forget it. Only a check that finds
nothing newer withdraws it.

| command          | while locked | does                                    |
| ---------------- | ------------ | --------------------------------------- |
| `update_status`  | open         | the snapshot                            |
| `update_check`   | open         | checks now, resolves with the result    |
| `update_install` | **refused**  | downloads, verifies, installs, restarts |

The badge on the Settings button shows while locked, like the library-health
badge and for the same reason: whoever is at the desk can see that an admin is
wanted. The button's tooltip carries the running version, since a locked desk
cannot open About and a bug report still needs it.

## Leaving cleanly

An update ends the process, and two things must not be lost with it:

- **The session.** The renderer flushes it before it asks for the install
  (`installUpdate` in `state.svelte.ts`), so the relaunched app restores the
  playlist it had.
- **The airing log and the now-playing "stopped".** `shut_down` in `lib.rs`
  drains the playlist's command queue and shuts the broadcast down. Every exit
  runs it from `RunEvent::ExitRequested`, and the restart after an install goes
  through that event.

**Windows is the exception.** Its installer cannot replace a running program,
so the updater plugin launches the installer and calls `process::exit` on the
spot — no exit event. The plugin's `on_before_exit` hook is the one chance, and
it runs the same `shut_down`.

## Two things the plugin does that the code works around

- **It looks up this installation's bundle before it compares versions.** A
  manifest without one fails the check even when nothing newer exists — which
  is every check against a release that failed to build for one platform. The
  version comparator runs first, so `update.rs` has it remember whether the
  manifest named something newer, and reads a missing bundle as either "up to
  date" or "newer, but not installable here".
- **It validates the whole manifest.** One malformed entry breaks the check for
  every platform. The website's build fails rather than publish one.

`requireSignedVersion` is on. The manifest is not signed, so without it a
tampered one could pass an old, validly signed bundle off as a newer version
and walk an install backwards. With it, the version in the signature's trusted
comment — which the signature covers — must match the version the manifest
announces. Every release since v0.25.2 carries one.
