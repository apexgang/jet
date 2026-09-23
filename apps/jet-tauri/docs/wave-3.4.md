# Wave 3.4: Parity and adaptation (Tauri / Linux)

The main window now keeps its size and maximized state across launches, on
the monitor it was on. It also remembers the destination, the work-panel tab,
whether the panel was open and both column widths. F11 enters full screen,
Ctrl+W closes the window and Ctrl+Q quits. Both columns can be resized with
the mouse or the keyboard. Below 1101 px the work panel becomes a focused
overlay that never opens on its own and never covers Send. The theme is
built from tokens that meet WCAG contrast in light and dark, with increased
contrast and forced colours on top. Shortcuts come from one model, and they
stay quiet while an IME composes or a dialog is open. New task can no longer
post into another task (D17). The cross-client parity matrix is in
`docs/desktop-parity-matrix.md`.

## Delivered

### Shortcuts and keyboard (S1)

- `src/lib/features/shell/shortcuts.ts` resolves every shortcut from one
  table. A shortcut never fires while an IME composes, a modal `<dialog>` is
  open or a key repeats. There are no bare-letter shortcuts. Only intents
  that have shipped are enabled. Settings › General › Keyboard shortcuts
  lists the same table. Ctrl+Alt+digit can collide with some window
  managers' workspace switchers; the arrow keys in the tablist work too.
- Ctrl+K focuses the Search field, and Ctrl+Shift+O opens Setup with focus
  on "Choose Folder…". Focus requests are one-shot.
- "Interrupt Turn…" and "Stop Run…" in the approval card open the Run tab,
  put focus on Cancel, and Escape cancels and returns focus to the card
  button (D6).
- The work-panel tabs follow the APG tabs pattern: roving tabindex, arrows,
  Home and End, and `aria-controls` only when the panel exists (D7).
- The timeline is no longer one live region. A visually hidden status line
  announces "Task status: …" (D8).
- **D17.** While New task, Search or a Project is open, a Plane event never
  selects a task, and the next Send creates a task.

### Theme (S2)

- Every colour is a token on `:root`, redefined for the light scheme and for
  `prefers-contrast: more` in both schemes. `forced-colors: active` has a
  system-colour rule for every disabled, focus, selected and status style.
  Nothing is translucent, and the `backdrop-filter` header is gone.
- `tests/theme-contract.test.ts` computes WCAG contrast for the token table
  in all four effective schemes. It fails on a colour literal outside a
  token block, a `backdrop-filter`, an `outline: none` without a
  forced-colours replacement, an animation without a reduced-motion rule,
  and an opacity-only disabled style.

### Layout (S3)

- `src/lib/features/shell/layout.ts` is the pure layout model. The
  conversation always keeps 420 px. The sidebar is 210–300 px and the work
  panel 280–440 px, the Swift ranges.
- At 1100 px and below, the work panel is an overlay dialog with a scrim.
  The sidebar and the main region are `inert` behind it. Escape, the scrim
  and Hide close it, and focus returns to what opened it. It opens only when
  the user asks: a toggle, a tab, a shortcut, a Run control or Needs
  attention. `WorkPanel` is never unmounted, so its state survives.
- Column separators are keyboard-operable (`role="separator"`, arrows,
  Shift for large steps, Home and End, double-click to reset).
- Every main-window destination header has the sidebar toggle, and F9
  toggles it.

### Window and layout persistence (S4)

- Five client-local commands: `load_shell_presentation` (both windows),
  `save_shell_presentation`, `toggle_main_window_fullscreen`,
  `close_main_window` (main only) and `quit_jet` (both windows). No
  `core:*` permission is granted.
- `window-geometry.json` and `shell-presentation.json` live in the app data
  directory, mode 0600, written atomically through one `local_store` writer
  and read with a size bound. They hold sizes, a maximized flag, a
  destination kind, two booleans, a tab and two widths. No Jet identifier or
  content. Nothing is in browser storage.
- Geometry is logical and clamped to a connected monitor. Full screen is not
  restored. Wayland does not let a client place its window, so position is
  restored on X11 only (PE-5).
- Startup follows the Swift order: the saved layout and 3.2's "Reopen the
  last task", then Setup (an incomplete setup wins), then Recent and the
  restored task. Anything the user does meanwhile wins.
- Settings › General › Restoration shows the window-layout state, including
  "Jet couldn't read the saved window layout, so it started with the default
  layout."

### Parity matrix and adaptation audit (S5)

- `docs/desktop-parity-matrix.md` has 110 capability rows seeded from every
  row of `docs/desktop-protocol-ui-matrix.md` and every public method in
  `packages/jet-client/src/requests`, with the mapping checklist, the
  adaptation configurations, the exceptions and the design-language
  coverage.
- `just adaptation-audit` (`scripts/adaptation/`) runs the real app in
  Chromium through `agent-browser` against mocked IPC. See Verification.

## Boundaries

- No protocol call was added. Window geometry and layout are client-local
  (`docs/desktop-protocol-ui-matrix.md`, "App appearance, window
  restoration, local notification preference").
- The Settings window can read the layout status and quit. It cannot save a
  layout or change the main window.
- 3.1 owns the Plane label and restoration of the selected task. 3.2 owns
  the Settings window, Ctrl+, and "Reopen the last task". 3.2 has no forced
  light or dark appearance yet, so the theme follows the system scheme only.

## Parity

The matrix has 110 rows. Linux: 77 Pass, 1 Gap, 10 Gap (both) under PD-1,
6 Exception, 7 Backend, 7 Pending, 2 n/a. Six of the Pending cells wait on
the manual native checklist below (checks 3, 4, 5, 10, 12 and 14); the
Linux column is verified by automated tests and the Chromium audit only. The macOS column was observed
read-only at `79d450a`. The Swift client stops at Wave 2.2, so 22 of its
cells read Pending.

Proposed exceptions and deferrals. **None is approved yet.** Wave 3.4
cannot close for Linux until the product owner decides each one.

| ID | Summary | Approval |
| --- | --- | --- |
| PE-1 | No GTK menubar; each menu command has a control and a shortcut, Help points to Settings › Diagnostics | Proposed |
| PE-2 | WebKitGTK 2.52.6 does not expose `prefers-reduced-transparency`; nothing is translucent | Proposed |
| PE-4 | Only if WebKitGTK does not report `prefers-contrast: more` for a desktop high-contrast setting | Not triggered; needs manual check 7 |
| PE-5 | Window position restored on X11 only | Proposed |
| PE-6 | "This computer" instead of "This Mac or the device name" | Proposed |
| PE-7 | No find inside a task (macOS has none either) | Proposed |
| PD-1 | Rename, fork, handoff, promotion, import, supervised and direct runs deferred on both clients | Proposed |

## Remaining backend dependencies

| ID | Missing surface | Evidence |
| --- | --- | --- |
| `plane_display_name` | A device name for a Plane | `PlaneStatus` has none (`packages/jet-protocol/src/message/mod.rs:167-195`) |
| `needs_attention_query` | A bounded Query of attention items across a Plane; attention precedence on relaunch | `docs/desktop-protocol-ui-matrix.md:43` |
| `conversation_layout` | Pinned tasks | `docs/desktop-protocol-ui-matrix.md:44` |
| `approval_decision_command` | A generic Harness approval decision | `docs/desktop-protocol-ui-matrix.md:57` |
| `plane_transfer_gui` | A GUI Query and transport for Plane transfer | `docs/desktop-protocol-ui-matrix.md:91` |
| `jet_client_remote_tool_review_query` | A public `jet-client` method for `remote_tool_review` | `packages/jet-client/src/connection/mod.rs:220` (`query` is `pub(crate)`) |
| Git delivery preview | Current branch, HEAD and remote URL bound to the confirmation | `apps/jet-tauri/docs/wave-2.3.md` |

## Platform notes

- **WebKitGTK 2.52.6** (Arch `webkit2gtk-4.1`): `libwebkit2gtk-4.1.so`
  contains `prefers-contrast`, `forced-colors`, `prefers-reduced-motion` and
  `prefers-color-scheme`, and not `prefers-reduced-transparency` (PE-2).
- **The development host runs Hyprland on Wayland, not GNOME.** The GNOME
  checks (appearance, High Contrast, `enable-animations`, the GTK key theme)
  need a GNOME session. `gtk-key-theme` reads `'Default'` and
  `enable-animations` reads `true` here.
- **Chromium is not WebKitGTK.** The audit proves CSS, layout logic and axe
  rules. It does not prove the engine, the production CSP or native window
  behaviour; those are in the manual checklist.
- Column widths are CSS custom properties set through CSSOM (Svelte
  `style:`), which the production `style-src 'self'` allows. Manual check 11
  confirms it in a release build.

## Verification

Run from `apps/jet-tauri` with the pinned Rust 1.98.1 toolchain:

```sh
bun install --frozen-lockfile
bun run check
bun run test
bun run build
cargo +1.98.1 fmt --manifest-path src-tauri/Cargo.toml --check
cargo +1.98.1 clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo +1.98.1 test --manifest-path src-tauri/Cargo.toml
just adaptation-audit
```

Results on 2026-09-23: svelte-check 0 errors and 0 warnings; Vitest 466
tests in 38 files (441 in node, 25 in the four happy-dom component files);
the static build succeeded; rustfmt and clippy were clean; Rust 288 tests
passed and 3 ignored. S5 changed no native code, capability or packaging,
so the debug `.deb` was not rebuilt for it.

### Adaptation audit

`just adaptation-audit` starts `bun run dev`, opens each scene in each
configuration and checks it. Scenes: `setup` (no Projects), `approval-run`,
`changes`, `new-task` (restore off), `planes`, `schedules`, `trash` and the
`settings` route. Configurations (26): seven viewports (900x600, 1100x700,
1101x700, 1280x800, 1920x1080, 2560x1440, 1280x800@2) in light and dark,
plus increased contrast (light, dark), forced colours (light, dark), reduced
motion and reduced transparency at 1280x800 and 900x600. The script refuses
to run when the list stops covering a required configuration.

Checks per run: axe has no serious or critical violation (`color-contrast`
is not counted under forced colours, where the system palette decides); the
window does not scroll sideways; the conversation keeps 420 px unless the
overlay covers it; in a compact window the work panel is closed after load
and Send is not covered; nothing animates after 1 s under reduced motion; no
element has a `backdrop-filter`; and a 40-step Tab walk where each focus
stop has an outline or box shadow (its own, or a `:focus-within` ring on its
parent that goes away on blur) and is not covered by another element.
Viewport and media features are emulated over the page's DevTools
connection and checked against `matchMedia`, so an emulation that did not
apply fails the run. Screenshots and `report.json` go to
`$TMPDIR/jet-adaptation/<ISO>/`, never the repository.

Result on 2026-09-23 (agent-browser 0.37.1, axe-core 4.12.1): **208 runs,
0 failed.** A run with a planted `backdrop-filter`, removed focus outlines
and an oversized column failed as expected.

Findings the audit raised, fixed in the owning files:

| Finding | Fix |
| --- | --- |
| `--quiet` text was 4.18:1 on the approval card's warning tint and 3.98:1 on a selected sidebar row (dark) | Dark `--quiet` is now `#89959c` (4.66:1 on `--selected`, 4.89:1 on the tint). The contract table now also checks `--text`, `--muted` and `--quiet` on `--hover` and `--selected` |
| The composer's only focus sign was a 1 px border colour change | `.composer-box:focus-within` adds a 1 px focus ring outside the border |
| No main landmark in the main window | The main region is `<main>` (still `display: contents`); the overlay test asserts it |
| Work-panel section headings were `<header>` elements, so axe read them as page banners | They are `<div>`s |
| `aria-label` on plain `div`s (context row, Run controls, checkpoint controls, recovery actions) and on the attention badges | The `div`s are `role="group"`. The badges carry visually hidden text ("2 items", "Needs attention") |
| The context row and Run details forced capitals on the Plane label ("This Computer", "Bytes") | The context row shows values as named; the Run details' "Runs on" keeps the label as named. Lifecycle and activity keep Swift's capitalization |

Warnings left in the report: axe `region` (moderate) for the column
separators, which sit between landmarks; and `color-contrast` on `kbd`
hints under forced colours, where the system palette decides.

The mocked IPC answers the main- and Settings-window load commands from
synthetic data derived from `fixtures/desktop/presentation-v1.json`. It
leaves `load_pairing` and `load_plane_detail` unanswered, so those sections
render their "unavailable" state in the audit.

### Manual native checklist

**Not run in this change.** Each item needs a person at a native desktop
session, and several need GNOME or two monitors; this host runs Hyprland
on one Wayland session. Record the date, desktop and result for each item
here and in the matrix.

| # | Check | Result |
| --- | --- | --- |
| 1 | 900x600 minimum; compact overlay | Pending |
| 2 | 1280x800 on first run | Pending |
| 3 | Size and maximized state restored on X11 and Wayland (position on X11 only) | Pending |
| 4 | Two monitors, mixed scale; unplug the secondary | Pending |
| 5 | F11 in and out; relaunch windowed; full screen on the secondary | Pending |
| 6 | GNOME dark and light: `prefers-color-scheme` and native dialogs | Pending (needs GNOME) |
| 7 | GNOME High Contrast reported as `prefers-contrast: more`? Decides PE-4 | Pending (needs GNOME) |
| 8 | `enable-animations false` reported as reduced motion | Pending (needs GNOME) |
| 9 | Large Text 1.25 at 900x600 | Pending |
| 10 | Keyboard-only run of every critical action; Orca spot check | Pending |
| 11 | Production CSP: resizer widths apply in `tauri build --debug` | Pending |
| 12 | Emacs key theme: what Ctrl+N, Ctrl+K and Ctrl+W do in the composer and Search | Pending (needs GNOME) |
| 13 | Corrupt `window-geometry.json` and `shell-presentation.json`: the window still shows, Settings shows the read-failed line | Pending |
| 14 | Ctrl+W with and without Settings open; Ctrl+Q from both windows; a Run keeps going across quit | Pending |

macOS was not verified by this Tauri-only change.

## Latent defects left for later

- Ctrl+W and Ctrl+Q save a layout change that is still inside the page's
  300 ms debounce before they close (bounded at 500 ms). The title-bar close
  button closes natively and does not wait, so a layout change made just
  before clicking it can be lost; fixing that needs the close to go through
  the page or the layout save to move native.
- `src/app.html` wraps the body in `style="display: contents"`. The
  production CSP (`style-src 'self'`) ignores the attribute; harmless because
  `.app-shell` sizes itself.
- Most buttons inside the work panel (Refresh, Apply, New, Save and others
  from Wave 2.2) have no class and render with the engine's default button
  style. They are focusable and labelled, so the audit passes; they need the
  shared button styles.
- The column separators sit outside every landmark (axe `region`,
  moderate).
