# Admin mode

A guest DJ running a show needs the decks, the playlist and the library. They
should not be able to reconfigure the station or change the library by
accident. Admin mode puts those actions behind a password.

Implements [#386](https://github.com/Arskah/radiodiodj/issues/386).

**This guards against mistakes. It is not a security boundary.** Anyone with
access to the data directory can edit `config.json` and remove the password.

## What is gated

While locked:

- The whole _Settings_ overlay: Audio Output, Library (paths, scan, library
  health, purge, recalculating automatic cue points), Playlist, Now Playing,
  Appearance (picking a theme, reloading themes, the station name and its
  images) and Advanced. The toolbar's Settings button is disabled, so none of
  it is reachable in the first place.
- Metadata edits: the _Edit metadata…_ row action and the row's edit button are
  hidden. Revert, retry and dismiss for tag writes live in the metadata editor
  and the health view, so they are out of reach too.
- _Save to track_ in the cue-point editor. It changes every future airing, the
  same way a metadata edit does.
- Cancelling a running scan, or the analysis pass behind it, from the status
  bar.

Still open while locked: playback, the playlist, library search and browsing,
the cue deck, _Show in folder_, and the cue-point editor's _Use once_ and
auditioning, which only affect one airing.

`library_check_now` is open too, even though the button that calls it sits
behind the disabled Settings overlay. A check reads the disk and reports; it
changes nothing and deletes nothing, so there is no reason for the backend to
refuse one. `ADMIN_COMMANDS` gates the commands that _change_ the library, and a
test pins `library_check_now` as deliberately outside it.

Reading the appearance is **not** gated, and cannot be: the renderer paints
itself from `get_appearance` before it mounts, on a launch that starts locked.
Only the commands that change it are in `ADMIN_COMMANDS`. See
[theming.md](./theming.md).

The library-health badge stays on the Settings button while locked. It tells
whoever is at the desk that an admin should log in.

## Locking and unlocking

- With no password set, admin mode is always unlocked. Behaviour matches
  installs from before admin mode.
- With a password set, every launch starts locked.
- The toolbar padlock unlocks, which asks for the password, or locks with one
  click. It shows only when a password is set.
- Admin mode locks again after _Settings → Advanced → Lock after idle_ minutes
  without input (default 15, 1–240).
- _Settings → Advanced → Admin Mode_ sets, changes or removes the password.
  Setting a password leaves the current session unlocked.

## Enforcement

The UI hides or disables gated actions, but the backend is what enforces the
lock. `run()` wraps the command handler in `admin_gated`, which checks every
invoke against `admin::ADMIN_COMMANDS` and rejects the call with
`admin mode is locked` while locked. Rejected commands never run, whatever
they return.

The unlocked flag lives in `AppState` (`AdminLock`) and starts false on every
launch. Only `admin_unlock` with the right password sets it, so the renderer
cannot unlock by itself. A new admin-only command must be added to
`ADMIN_COMMANDS`. A test checks that every name there is a registered command.

The idle timer runs in the renderer (`features/admin/idleLock.ts`), because
only the renderer sees input. When it runs out it calls `admin_lock`. Locking
from the renderer is harmless.

## The password

`config.json` stores an Argon2id hash in PHC format, never the plaintext:

```json
{
  "admin": {
    "passwordHash": "$argon2id$v=19$m=19456,t=2,p=1$…",
    "idleLockMin": 15
  }
}
```

A hash that does not parse keeps the app locked.

### Forgot the password

1. Quit RadiodioDJ.
2. Open `config.json` in the data directory (see the README's _Data files_).
3. Delete the `passwordHash` line from the `admin` section.
4. Relaunch. Admin mode is now always unlocked; set a new password in
   _Settings → Advanced_.
