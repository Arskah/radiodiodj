# Theming — colour schemes and station identity

An operator retints the whole player by picking a **theme**, or by dropping
their own folder into the app data directory. The station's name and artwork are
set separately, and survive every theme they try.

Implements [#403](https://github.com/Arskah/radiodiodj/issues/403).

Three rules hold throughout:

- **A theme only paints.** It sets colours. It never changes layout, copy,
  fonts or behaviour.
- **A bad theme is reported, never half-applied.** An invalid theme is listed
  and disabled with the reason. An _incomplete_ one is filled in from its base.
- **The app never repaints itself unasked.** No filesystem watcher, no
  following the OS appearance, and a theme that breaks while the app is running
  leaves the colours on screen alone.

## What a theme is

A theme is a **directory** under `{app_data_dir}/themes/`, holding a
`theme.json` and whatever images it ships:

```text
~/Library/Application Support/com.radiodiodj/
  themes/
    example/
      theme.json
    station-red/
      theme.json
      logo.svg
      label.png
```

```json
{
  "name": "Station Red",
  "author": "Radio Foo",
  "base": "dark",
  "tokens": {
    "--primary": "#ff5b5b",
    "--on-primary": "#380000",
    "--primary-container": "#c40000"
  },
  "logo": "logo.svg",
  "label": "label.png"
}
```

Only `name`, `base` and `tokens` are required, and `tokens` may be as short as
one entry. Unknown top-level keys are ignored, which is what lets a theme carry
notes for its author (see [Authoring](#authoring-a-theme)); keys inside `tokens`
are the opposite, and are checked strictly.

**A theme is a directory, not a loose file.** Assets belonging to different
themes would otherwise collide on filenames in one shared folder; a directory is
what an operator zips, hands to a colleague, or drags to the bin; and
enumeration is one rule — read `themes/`, keep the entries containing a
`theme.json` — rather than two. The cost is one empty folder for a
colours-only theme.

**Identity is the directory name.** `theme.json` carries no `id` field. The
filesystem already enforces uniqueness, a name can never drift from the thing it
names, a duplicated folder is automatically a distinct theme, and there is no
"two files claim `station-red`" reconciliation rule to write or test.
`config.json` stores `appearance.themeId: "station-red"`, which reads correctly
in a text editor.

**Built-ins are theme files.** `src-tauri/themes/midnight.json` and
`daylight.json` are compiled in with `include_str!` and go through the same
parser, the same validator and the same merge as an operator's theme. There is
one theme model, so a shipped theme and a dropped-in one cannot diverge in
shape. `midnight` and `daylight` are reserved names: a theme directory using one
is listed as invalid rather than silently shadowing or being shadowed.

A test asserts each built-in parses, validates, and is **token-complete** — a
built-in is what others fall back to, so it may not have holes.

## Authoring a theme

On first launch, if `themes/` does not exist, it is created holding one
`example/` theme: every token in the contract set to its Midnight value, named
_Example (copy me)_. The loop is then:

1. _Settings → Appearance → **Show in folder**_.
2. Copy `example/` to a folder named for the new theme.
3. Edit `name`, and the values that should change. Delete the tokens that
   should not — anything omitted comes from the base.
4. _Settings → Appearance → **Reload themes**_.
5. Pick it from the list.

Seeding happens only when `themes/` is **absent**, never when it is merely
empty, so deleting the example does not resurrect it.

The example is a valid, ordinary theme identical to Midnight, so it lists and
selects like any other — which proves the loop works before a single value is
changed.

JSON has no comments, so the example's guidance rides in a top-level `"notes"`
array, which the parser ignores:

```json
{
  "name": "Example (copy me)",
  "base": "dark",
  "notes": [
    "Values are #rgb, #rrggbb, #rrggbbaa, rgb(), rgba(), hsl(), hsla(),",
    "or the keyword transparent. Nothing else.",
    "Delete any token you don't want to change — it comes from the base."
  ],
  "tokens": {}
}
```

Guidance cannot live inside `tokens`, whose keys are allowlisted; a comment
there would be reported as an unknown token. JSONC was considered so the example
could carry real comments, and dropped: `serde_jsonc` is a fork of `serde_json`
frozen at upstream 1.0.108 with no release since October 2023, which would put
two copies of the parser in the build with the stale one handling
operator-supplied files, and `jsonc-parser` is a sound dependency bought for
comments alone. `"notes"` costs nothing.

## The token contract

A theme may set these, and nothing else:

| group       | tokens                                                                                                                                                                                                                           |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| surfaces    | `--surface`, `--surface-dim`, `--surface-bright`, `--surface-container-lowest`, `--surface-container-low`, `--surface-container`, `--surface-container-high`, `--surface-container-highest`, `--surface-variant`, `--background` |
| content     | `--on-surface`, `--on-surface-variant`, `--outline`, `--outline-variant`                                                                                                                                                         |
| primary     | `--primary`, `--on-primary`, `--primary-container`, `--on-primary-container`, `--inverse-primary`                                                                                                                                |
| secondary   | `--secondary`, `--on-secondary`, `--secondary-container`, `--on-secondary-container`                                                                                                                                             |
| tertiary    | `--tertiary`, `--tertiary-container`, `--on-warning-container`                                                                                                                                                                   |
| error       | `--error`, `--on-error`, `--error-container`, `--on-error-container`                                                                                                                                                             |
| signal      | `--signal-green`, `--led-highlight`                                                                                                                                                                                              |
| cue markers | `--cue-in-color`, `--fade-in-color`, `--fade-out-color`, `--cue-out-color`, `--next-start-color`, `--on-marker`                                                                                                                  |
| depth       | `--shadow-color`, `--inner-highlight`, `--scrim`                                                                                                                                                                                 |
| controls    | `--on-toggle-knob`                                                                                                                                                                                                               |

The same names are the `:root` block at the top of `src/styles.css`, the
`THEMEABLE_TOKENS` list in `appearance/theme.rs`, and the key set of every
built-in. A token missing from any of the three fails the contract guard.

**Colours only.** Spacing (`--sp-*`), radius (`--r*`) and the two font tokens
are deliberately not themeable. No colour a validator accepts can break the
layout, and the SVG waveform and cue markers retint for free because they are
CSS classes over `var()` references. `--sp-md: 40px` breaks the toolbar and the
grid, and no validator can tell a working geometry value from a breaking one
without rendering it. Fonts additionally need font files, a `@font-face` rule
built from operator text, and a fetch — every property colours were chosen to
avoid.

**Shadows are colours, not shadows.** `--shadow-color`, `--inner-highlight` and
`--scrim` are colours; the geometry (`inset 0 2px 4px`) stays in `styles.css`.
This is what a light theme actually needs — black shadows read wrong on a light
surface — while keeping the value grammar to exactly one production. A themeable
`box-shadow` would have meant a second, much looser grammar for the one case
that does not need it.

**A shadow token carries no alpha of its own.** Each site mixes the opacity it
wants — `color-mix(in srgb, var(--shadow-color) 55%, transparent)` — so the
fourteen shadows in `styles.css` keep the four distinct alphas they were tuned
with, and one token still retints all of them. Baking the alpha into two tokens
(`--shadow-color` and a `-strong`) was the first shape of this, and it would
have quietly flattened 0.4 / 0.5 / 0.55 / 0.6 into two values. `--scrim` is the
exception and carries its own alpha, because it is used directly as a
background rather than inside a shadow.

There is no `--glow-primary`. The primary glow is
`color-mix(in srgb, var(--primary) 40%, transparent)`, which is what the
hardcoded `rgba(184, 195, 255, 0.4)` already was, so it follows `--primary` with
no token of its own.

**The set stays small because `color-mix` does the rest.** `styles.css` already
uses `color-mix(in srgb, var(--token) 25%, transparent)` in 49 places, so every
derived tint follows its source token. That is the reason not to publish a token
per shade.

### Base

`base` is `"light"` or `"dark"`, and is required. It does two things: it names
the built-in that fills in whatever the theme left out, and it drives native
chrome. Applying a theme sets

```js
document.documentElement.dataset.themeBase = base;
document.documentElement.style.colorScheme = base;
```

`color-scheme` is the only thing that reaches scrollbars, the `<select>` popup,
form controls and autofill — no custom property can. `data-theme-base` is the
escape hatch for CSS that legitimately cannot be a colour token, such as a
different shadow _geometry_ on light.

The app does **not** follow `prefers-color-scheme`. A broadcast player's palette
must not change at sunset because the OS decided to. The operator picks a theme;
that is the feature.

## Validation

Validation runs in **Rust, at load, once**. The renderer never sees an
unvalidated token. One implementation cannot drift from a second, it runs before
anything reaches the DOM, and a bad theme produces one reportable error in the
theme list instead of a UI that is quietly wrong in a way nobody can attribute.

**Rule 1 — the token name must be in `THEMEABLE_TOKENS`.** An unknown key is an
error, not an ignore: `--on-surfase` would otherwise silently do nothing and the
author would blame the app.

**Rule 2 — the value must parse as one of these, and nothing else:**

| form                        | example                                   |
| --------------------------- | ----------------------------------------- |
| hex, 3 / 4 / 6 / 8 digits   | `#f33`, `#ff5b5b`, `#ff5b5b80`            |
| `rgb()` / `rgba()`, numeric | `rgb(255 91 91)`, `rgba(255, 91, 91, .4)` |
| `hsl()` / `hsla()`, numeric | `hsl(0 100% 68%)`                         |
| the keyword `transparent`   | `transparent`                             |

No `var()`, no `color-mix()`, no `url()`, no `oklch()`, no named colours. Named
colours are excluded because `chocolate` and a typo are indistinguishable to
anyone reading a diff, and "write `#d2691e`" is a better outcome. `oklch()` is
excluded for v1 because Rust accepting a value and the webview painting it are
two different statements: an unsupported colour function makes `setProperty` a
silent no-op, and that token falls through to the base — the half-applied
palette this design refuses. Hex, `rgb()` and `hsl()` are universal, so
"validated" and "painted" mean the same thing.

The grammar cannot express `;`, `}`, a whitespace-separated second declaration,
or a fetch. The injection surface closes at the grammar rather than at an
escaping function.

### Invalid, or merely incomplete

Two different things, two different outcomes.

|                                       |                                                                                                                                                                                                                                                                             |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Invalid** → refused whole           | Malformed JSON, an unknown token name, a value outside the grammar, an asset that does not resolve inside the theme's own directory, a reserved name. The theme is _listed_ in Settings, greyed and disabled, with the reason and the offending key. It cannot be selected. |
| **Incomplete** → filled from the base | A theme declaring six tokens is perfectly valid. The rest come from the built-in named by its `base`. This is the common case.                                                                                                                                              |

Per-token fallback for an _invalid_ theme was rejected: a UI that is 90 %
Station Red and 10 % Midnight is harder to diagnose than a refusal, and "why is
that one button still blue" is a worse bug report than "Station Red:
`--surfase` is not a theme token".

### Assets

`logo` and `label` are file names resolved against the theme's own directory,
then canonicalized and required to `starts_with` it — the same root-membership
rule `library/listing.rs` applies to library paths, so `../../../etc/passwd` and
a symlink escape fail the same test. Extensions are limited to `svg`, `png`,
`jpg`, `jpeg` and `webp`, raster formats are magic-byte sniffed, and the cap is
2 MiB.

**A theme asset is never inlined into the DOM.** It is always
`<img src="data:…">`. Scripts inside an SVG loaded through `<img>` do not
execute and external references do not resolve, which is what makes shipping SVG
safe. This rule is load-bearing; do not relax it to get styleable SVG.

### Posture

The same as [admin mode](./admin-mode.md): **not a security boundary.** Anyone
who can write to `{app_data_dir}/themes` can already write `config.json` and the
database. Validation exists so that a _mistake_ is reportable, and so that a
malformed theme cannot leave the UI unusable or make a network request — not to
defend against someone already inside the data directory.

## Applying a theme

**The backend resolves; the renderer only paints.** `get_appearance` returns a
complete, already-merged, already-validated token map, so the renderer carries
no copy of any built-in palette and no merge rule:

```ts
private applyAppearance(next: Appearance): void {
  const root = document.documentElement;
  for (const key of this.appliedTokens) {
    if (!(key in next.tokens)) root.style.removeProperty(key);
  }
  for (const [key, value] of Object.entries(next.tokens)) {
    root.style.setProperty(key, value);
  }
  this.appliedTokens = Object.keys(next.tokens);
  root.dataset.themeBase = next.base;
  root.style.colorScheme = next.base;
  this.appearance = next;
}
```

**Inline custom properties on `documentElement`, not an injected `<style>`
block.** A style block is string concatenation of operator text into CSS —
exactly the surface the validator exists to close, re-opened as a second line of
code. Inline properties beat the `:root` block with no specificity games and no
`!important`, clearing one is `removeProperty`, and `setProperty` goes through
the CSSOM, so a value that somehow got past the validator is a no-op rather than
a parse of its surroundings. Static `[data-theme]` blocks in `styles.css` were
the third option; they cannot express file themes at all.

### Startup order

`styles.css` is Midnight, and `main.ts` mounts immediately, so for an operator
on a light theme the first painted frame would be a dark flash on every launch.
Three parts fix it:

1. **`main.ts` awaits appearance before `mount`.** It is the one awaited load;
   everything else keeps its `void`. The cost is one `invoke` round trip over a
   blob already resident in Rust memory.
2. **A synchronous paint hint.** Before that await, the renderer reads
   `localStorage["appearance-paint"]` — `{ base, background, surface }` — and
   sets three properties plus `colorScheme`, removing even the one-round-trip
   dark frame. This is the only renderer-side persistence in the app, and it is
   acceptable because **it is a paint hint, never a source of truth**: nothing
   reads it once the real load lands, and if it is missing, stale or garbage the
   worst case is the flash it exists to prevent.
3. `styles.css` gets `html { background: var(--background) }` and
   `:root { color-scheme: dark }`, so the pre-module canvas is not white.

If `get_appearance` fails outright, the app mounts anyway on the `:root`
Midnight fallback and logs. Appearance must never stop the app from starting.

## Reload

Two explicit triggers, and no watcher:

1. **Opening _Settings → Appearance_** re-enumerates. Covers "I dropped a folder
   in, then went to look for it".
2. **_Reload themes_** re-enumerates _and_ re-resolves the active theme. Covers
   "I just edited the theme I'm using".

[library-health.md](./library-health.md) already argues for a timer over a
watcher; here the case is different and stronger. The operator is standing at
the machine, and a watcher would repaint the live UI from a half-written file
the moment an editor saves — mid-broadcast, unasked. A repaint is an ask.

| situation                                                        | what happens                                                                                                                                                     |
| ---------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| active theme still valid                                         | new tokens resolved and applied; the list refreshes                                                                                                              |
| active theme now invalid, or its folder is gone, **app running** | **the palette on screen does not change.** The theme is listed greyed with the reason, and the tab says so: _"…— the colours on screen are the last good ones."_ |
| active theme unresolvable **at launch**                          | falls back to `midnight`, one `log::warn!`, and the tab says so. `config.json` keeps the id, so fixing the folder and relaunching restores it.                   |

The running-app rule is the mid-show safety rule, and the same instinct as _an
unreachable library path prunes nothing_: a read that fails does not get to
destroy state the operator is depending on.

## Station identity

**Station identity is configured separately from the theme, and a theme may
ship _default_ images that identity overrides.** A palette does not know what
the station is called, and a station wanting light and dark variants of its own
colours should not have to duplicate its logo into two folders or watch its name
vanish when it tries the other one. Both stories still work: drop in a complete
station theme and it looks right immediately; set a logo in Settings once and it
survives every theme tried afterwards.

One precedence chain per slot:

```text
toolbar logo :  identity.logo        ?? theme.logo  ?? station name as text ?? "RadiodioDJ"
record label :  track cover art      ?? identity.label ?? theme.label ?? radiodiodi_label.svg
station name :  identity.stationName ?? APP_NAME     (a theme never sets this)
```

**Two image slots, not one.** `.brand` in the toolbar wants something wide and
short. `.vinyl-art` is `border-radius: 50%; object-fit: cover` — it crops to a
circle and centre-crops a wordmark badly, which is exactly why the shipped
`radiodiodi_label.svg` is a self-contained cream disc. One slot would force one
image to fail one surface. `object-fit: contain` on the label was rejected: a
transparent gap over the record grooves looks worse than a crop. A **Record
label** should be square, with anything important inside the inscribed circle.

### The toolbar

The logo **replaces** the station name; it never sits beside it.

| logo | station name | `.brand` renders  | `alt`        |
| ---- | ------------ | ----------------- | ------------ |
| —    | —            | text `RadiodioDJ` | —            |
| —    | `Radio Foo`  | text `Radio Foo`  | —            |
| set  | —            | `<img>`           | `RadiodioDJ` |
| set  | `Radio Foo`  | `<img>`           | `Radio Foo`  |

One rule, four states, no fourth layout. The name a logo replaces survives as
its alt text and in the window title. A station with a symbol-only logo that
also wants its name visible sets the name and clears the logo.

`styles.css` caps the image (`height: 20px; width: auto; max-width: 160px;
object-fit: contain`) so a 4000 px PNG cannot blow out the nav bar. That is the
one piece of geometry this feature needs, and it belongs in the stylesheet, not
in a theme.

### `APP_NAME` versus the station name

`APP_NAME` (`src/shared/appName.ts`) stays the **product** name and keeps
everything that identifies the software: the bundle, the dock entry, the log
file, `tauri.conf.json`. It is also the fallback for every station-name site, so
`appName.ts` is not modified.

The station name is **operator-facing chrome**: the toolbar, and the
`document.title` writes in `state.svelte.ts`, which move from `APP_NAME` to a
derived `app.brandName`. The window title therefore follows the station name;
the dock and taskbar entry stay `RadiodioDJ`, because those come from the
bundle. The backend trims the name and caps it at 64 characters, and returns the
normalised value.

### How an image reaches the webview

As a **base64 data URL over IPC**, exactly as `get_cover_art` →
`library::scanner::read_cover_art` already does: read the bytes, sniff the MIME,
encode, `format!("data:{mime};base64,{encoded}")`.

`asset://` is not available and is deliberately not being enabled:
`tauri.conf.json` sets `"csp": null` and declares no `assetProtocol`, the `tauri`
dependency enables no features (so no `protocol-asset`), and
`capabilities/default.json` grants `core:default`, `core:window:allow-destroy`,
`dialog:default` and `log:default` and nothing else. The data URLs ride along
inside the `Appearance` payload rather than taking their own command — they are
small, they are needed at first paint, and a second round trip would reintroduce
the flash.

An operator's chosen image is **copied into `{app_data_dir}/branding/`**, and
config stores only the file name, never a path. Nothing else in the app copies
an operator-chosen file in, so the reason is worth stating: a logo is a few
kilobytes and must be present at every launch, unlike a library, so owning the
copy is cheaper than depending on a path the operator may move. It also means a
theme asset and an identity asset are validated by the same rules over a
directory the app owns.

## Wire and storage

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceConfig {
    #[serde(default = "default_theme_id")]
    pub theme_id: String,
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,   // file name inside {app_data_dir}/branding/
    #[serde(default)]
    pub label: Option<String>,
}
```

`AppConfig` gains `#[serde(default)] pub appearance: AppearanceConfig`, per the
additive-schema convention — no version bump. One section rather than two, even
though the model keeps palette and identity independent: they are configured on
one tab and read by one command, and splitting them would put two locks and two
round trips in the path of one screen. The independence is a rule about
behaviour — switching theme never touches the identity fields — not about
storage layout.

```ts
interface Appearance {
  themeId: string;
  base: "light" | "dark";
  tokens: Record<string, string>; // complete, resolved, validated
  stationName: string | null;
  logo: string | null; // data URL
  label: string | null; // data URL
  problem: string | null;
}
```

`problem` is how the launch fallback and the failed-reload case reach the UI
without a second command.

| command                         | admin-gated | returns             |
| ------------------------------- | ----------- | ------------------- |
| `get_appearance`                | no          | `Appearance`        |
| `list_themes`                   | no          | `Vec<ThemeListing>` |
| `set_theme(themeId)`            | yes         | `Appearance`        |
| `set_station_name(name)`        | yes         | `Appearance`        |
| `set_station_image(slot, path)` | yes         | `Appearance`        |
| `clear_station_image(slot)`     | yes         | `Appearance`        |
| `reload_themes`                 | yes         | `Appearance`        |
| `reveal_themes_dir`             | yes         | `()`                |

**Every mutator returns the resolved `Appearance`**, so the renderer adopts what
the backend applied rather than what it asked for — the `set_tuning_config` and
`set_cue_points` echo pattern, and what keeps normalisation, base-merging and
fallback-on-failure single-sourced.

**The two readers are ungated by necessity**, not merely for consistency with
the rest of `ADMIN_COMMANDS`: `main.ts` calls `get_appearance` before `mount`,
on a launch that starts locked. `reload_themes` is gated because it repaints;
the tab is unreachable while locked anyway, so that gate is belt-and-braces.

**No event.** Every appearance change is renderer-initiated from Settings, and
the return value carries the result. There is no second window and nothing on
the backend that changes appearance on its own — unlike the idle lock, which is
why `admin-state-changed` exists. Following the OS appearance, or a second
window, would be what forces an event.

## Settings → Appearance

The tab sits between _Now Playing_ and _Advanced_: Audio, Library and Now
Playing are what the station does, Appearance is an ordinary operator setting,
and Advanced stays last as the tuning drawer.

```text
┌ Appearance ───────────────────────────────────────────────┐
│ Theme                                     [Reload themes] │
│  ◉ Midnight       Built-in · dark                         │
│  ○ Daylight       Built-in · light                        │
│  ○ Station Red    themes/station-red · dark               │
│  ⊘ Broken Blue    themes/broken-blue                      │
│      "--surfase" is not a theme token                     │
│  Themes live in …/com.radiodiodj/themes  [Show in folder] │
│                                                           │
│ Station identity                                          │
│  Station name  [ Radio Foo          ]  Shown in the       │
│                                        toolbar and the    │
│                                        window title.      │
│  Toolbar logo  [▭ preview ]  [Choose…] [Clear]            │
│  Record label  [◉ preview ]  [Choose…] [Clear]            │
│      Square, and keep anything important inside the       │
│      circle — cover art wins when a track has it.         │
└───────────────────────────────────────────────────────────┘
```

**Selecting a theme applies it immediately — the app _is_ the preview.** There
is no preview pane: a swatch strip that disagrees with the running UI is a bug
farm, and a 40-token palette cannot be honestly previewed in a thumbnail.
Reverting is selecting the previous entry.

**Invalid themes are listed, never hidden.** A theme that "didn't show up" is
the worst failure mode for a drop-in-a-folder feature; listed and disabled with
the reason turns it into a fixable message.

## Not built

- **Following `prefers-color-scheme`.** Deliberate — see
  [Base](#base). The hook if it is ever wanted is an `"auto"` pseudo-id
  selecting between two configured themes.
- **Per-token partial application** of an invalid theme.
- **A filesystem watcher** on `themes/`.
- **Themeable fonts and spacing.** Both need a validator that can tell a working
  geometry value from a breaking one; fonts additionally need font files and a
  fetch.
- **`oklch()` and named colours.** `oklch()` is one more arm in the same parser
  whenever webview support is no longer a question, or whenever the backend is
  willing to convert it to sRGB hex at resolve time so that "validated" and
  "painted" stay the same statement.
- **The OS / window icon and the vinyl disc.** The bundle icon is baked at build
  time from `src-tauri/icons/`; there is no per-window `icon` key, so a runtime
  swap needs `window.setIcon()` or regeneration. The `.vinyl-disc` background
  stays a stylesheet `url()`.
- **`:root[data-platform="darwin"] #toolbar`** (`styles.css:176`) is dead —
  nothing sets the attribute, so the macOS traffic-light inset does not
  currently apply. Left alone here: it is window-chrome padding, not a palette
  concern, and wiring it needs a platform signal this feature does not otherwise
  want.
- **Importing a theme from a zip.**

## Accepted limits

- A wide image used as a **Record label** is cropped to the circle.
- A light theme shows one dark frame if the paint hint is cold — first launch,
  or after the browser storage is cleared.
- An edited theme needs _Reload themes_; nothing repaints on its own.
- Data-URL images live in memory for the session.
- `config.json` keeps a `themeId` whose folder is gone, so restoring the folder
  restores the theme.

## Why it is built this way

- **Built-ins are theme files** — one model, one parser, one merge rule, so the
  shipped palette and an operator's cannot diverge in shape.
- **Colours only** — no accepted value can break the layout, and the SVG
  waveform and cue markers retint with no JS.
- **Refusal, not per-token fallback** — a half-applied palette is harder to
  diagnose than a refusal with the offending key named.
- **Inline custom properties, not injected CSS** — concatenating operator text
  into a stylesheet is the surface the validator exists to close.
- **A directory per theme** — assets cannot collide, and a theme is one thing to
  move, zip or delete.
- **Identity separate from the palette** — a palette does not know what the
  station is called.
- **Instant apply instead of a preview** — the running app is the only honest
  preview of a 40-token palette.
- **No event** — every change is renderer-initiated, and the mutator's return
  value carries the result.
