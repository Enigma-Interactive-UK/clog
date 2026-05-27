# Zen mode - design spec

> Dated 2026-05-27. Sourced from GitHub issue #2
> ([Feature Request] - VSC-Like Zen-Mode by @kaelyx-dev).

## Goal

Give the user a distraction-free view of the log records by hiding the
app's surrounding chrome. Modelled on VS Code's zen mode, but adapted to
Clog's UI and reduced to the smallest scope that satisfies the issue.

## Requirements (from the issue, restated)

1. Hide ancillary UI; show only the log records area.
2. Do **not** maximize or fullscreen the OS window.
3. `Ctrl+Tab` / `Ctrl+Shift+Tab` tab switching must keep working.
4. Must **not** persist across app restarts.
5. Must have a visible exit affordance in addition to the keyboard
   shortcut, so a user who forgets the shortcut can still leave.
6. Bound to a keyboard shortcut.

## What is hidden vs. kept

**Hidden in zen:**

- `AppHeader` (top toolbar)
- `TabStrip`
- `SearchBar` (bottom)
- `StatusBar` (bottom)
- `UpdateBanner` (treated as chrome - a banner popping in defeats zen)

**Kept in zen:**

- `LogViewport`, including its minimap and speed rail. The minimap is
  part of the log-viewing surface, not chrome, and is the primary
  navigation primitive inside the viewport.
- Transient signal that the user explicitly opted into or that they need
  to dismiss:
  - The error banner (`section.error` in `App.vue`) - it is dismissable
    and represents a real problem the user must see.
  - The rotation toast (`div.rotation-toast`) - short-lived, important.
- `RecordModal`, `PatternModal`, `SettingsModal`, `AboutModal`,
  `ContextMenu`, `DropOverlay`: these are all user-invoked overlays. If
  the user opens one while in zen, render it as normal. Zen does not
  block interaction, it only hides chrome.

## Keyboard binding

- **Toggle:** `F11`.
- **Exit also via:** `Esc` (only when zen is active and no modal is
  open), and the floating exit pill (see below).
- `Ctrl+Tab` / `Ctrl+Shift+Tab` continue to work unchanged - the
  shortcut handlers in `useAppShortcuts` do not touch chrome rendering.

### F11 default behaviour

Clog does not bind F11 today, and the Tauri shell does not surface a
default F11 action either. As a defensive measure the handler in
`useZenMode` still listens in the capture phase (matching the pattern
in `useAppShortcuts`) and calls `preventDefault()` +
`stopPropagation()` on the F11 keydown, so any future webview default
(e.g. WebView2's fullscreen behaviour kicking in) cannot fight the
toggle. This is the same technique already used to suppress `Ctrl+F`
and `F3`.

## Exit affordance

A small `ZenExitPill` component, position `fixed` in the top-right
corner of the viewport area, visible only when zen is active.

- Renders a single `<button>` labelled `Exit zen mode (F11)`.
- Default opacity ~0.5 so it does not compete with the logs; full
  opacity on `:hover`, `:focus-visible`, and during pointer movement
  near it.
- `aria-label="Exit zen mode"`, focusable, keyboard-activatable
  (`Enter` / `Space`).
- Click or activation calls the same toggle the keyboard binding does.
- Positioned with a small inset (e.g. `top: 0.5rem; right: 0.5rem`)
  so it does not overlap the minimap, which lives on the right edge of
  the viewport. Final inset to be tuned in implementation against a
  real screenshot.

## Persistence

None. `useZenMode` exposes a `ref(false)` initialised fresh on every
app mount. Not written to `settings`, not part of the session JSON, not
read on startup.

## Architecture

```
ui/src/composables/useZenMode.ts    NEW
  - exports { zen, toggle, enter, exit }
  - installs a capture-phase keydown listener for F11 (toggle) and
    Esc (exit only)
  - keydown handler is a no-op when an input/textarea/contenteditable
    is focused (matches user expectation that Esc inside a search box
    blurs the box, not toggles zen)

ui/src/components/ZenExitPill.vue   NEW
  - one button, fixed position, fade-on-idle styles
  - emits @click; parent toggles zen

ui/src/App.vue                      MODIFIED
  - import useZenMode -> destructure { zen, toggle }
  - wrap <AppHeader>, <TabStrip>, <SearchBar>, <StatusBar>,
    <UpdateBanner> blocks with v-if="!zen"
  - render <ZenExitPill v-if="zen" @click="toggle" />
  - error banner and rotation toast remain unconditional
```

`useAppShortcuts` is **not** modified. Keeping zen's keyboard handling
in its own composable preserves a single-responsibility boundary and
makes the feature trivially removable if it ever needs to be reverted.

## Edge cases

- **Modal open at toggle time:** zen toggles regardless. Modals
  continue to render normally; closing them returns the user to the
  bare viewport.
- **No tab open (`currentTab` is null):** entering zen still works,
  but the placeholder text becomes the only visible content. Exit pill
  still appears so the user can leave.
- **Esc with a focused search box:** the keydown handler short-circuits
  when the active element is an `input`, `textarea`, or
  `[contenteditable]`. Search box keeps its existing Esc-to-blur
  semantics; zen exits only when Esc is pressed against the document
  body or the viewport.
- **F11 in a modal:** zen still toggles. F11 is rare enough inside
  modals that this is acceptable, and the alternative (suppressing it)
  surprises the user more than it helps.

## Testing

- **Vitest unit tests for `useZenMode`:**
  - `toggle()` flips state.
  - F11 keydown toggles when document.body is focus target.
  - F11 keydown toggles when active element is an input (it is a
    non-text shortcut, so it should still fire).
  - Esc keydown exits zen when zen is on and no input is focused.
  - Esc keydown is a no-op when zen is on and an input is focused.
  - Esc keydown is a no-op when zen is off.
  - F11 keydown receives `preventDefault()` / `stopPropagation()`.
- **Smoke (component-level):** mount `App.vue`, assert
  `AppHeader` / `TabStrip` / `SearchBar` / `StatusBar` /
  `UpdateBanner` exist when `zen` is false, and are absent when
  `zen` is true. Assert `ZenExitPill` exists only when zen is true.

## Out of scope (v1 of this feature)

- Per-file or per-tab zen state.
- Animations / transitions when chrome appears or disappears - hard cut
  is simpler and matches the immediate "get out of my way" intent.
- A Settings entry. The shortcut and the issue-flag tooltip on the exit
  pill are enough discoverability for v1.
- Hiding the minimap. If demand surfaces, add a sub-toggle later; see
  `docs/future-ideas.md` for the place to file it.

## Files touched

- `ui/src/composables/useZenMode.ts` - new
- `ui/src/components/ZenExitPill.vue` - new
- `ui/src/App.vue` - modified (imports, chrome wrapping, pill render)
- `ui/src/composables/useZenMode.spec.ts` - new (vitest)
- Optional: `ui/src/App.spec.ts` if a smoke test is added

## Issue link

Closes [#2](https://github.com/Enigma-Interactive-UK/clog/issues/2).
