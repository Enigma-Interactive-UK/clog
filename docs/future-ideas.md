# Clog - Future ideas

> Snapshot dated 2026-05-23. Captured after the v1 build phases were laid out
> in `docs/build-phases.md`. These are candidates for v1.1 and beyond, not
> commitments. Re-evaluate each one after v1 has been used in anger.
>
> Grouped by theme rather than priority. The "Top picks" section at the
> bottom names the three that compound best on the existing foundation.

## Reading & navigation

- **Minimap / heatmap gutter** showing record density and error/warning
  clusters across the whole file. One of the highest-leverage features for
  "where did things go wrong" scanning. Cheap once the index exists.
- **Bookmark / pin records** with keyboard shortcuts and a sidebar list.
  Survives session restore (P7 already does the heavy lifting).
- **Time-axis ruler** - thin gutter showing wall-clock gaps between records,
  so big silences and bursts pop visually.
- **Go-to-timestamp** ("jump to 14:32:01") via binary search on
  `RecordHeader.timestamp`.
- **Collapse stack traces** behind a one-line summary; expand on click.
  P5 already classifies stack frames.

## Search beyond v1

- **Saved search / filter presets** per file pattern - "ERRORs from `play`
  only", recallable from a dropdown.
- **Search history** (deferred in P6; worth revisiting - cheap given
  persistence lands in P7).
- **Field-scoped operators** (`level:ERROR thread:akka msg:"connection
  refused"`). The parser already produces fields, so this is mostly UI plus
  a small query language.
- **Diff two records / two ranges** side-by-side.

## Multi-file power

- **Merge view across tabs** by timestamp - hugely valuable when correlating
  app + GC + access logs.
- **Saved workspaces** (a named set of tabs + filters + layout).
- **Split panes** is already noted for v1.1 in `design.md §11`. Prioritise
  it; pairs naturally with merge view.

## Analysis / insights

- **Per-logger and per-level histograms** in a collapsible side panel,
  scoped to the current filter. Click a bar to filter.
- **Error-rate sparkline** for the visible window, with anomaly markers.
- **"Show similar records"** - cluster records by msg shape (mask numbers,
  hex, UUIDs) so repeating noise collapses.
- **Custom user-defined thread groups** - v1 ships a fixed taxonomy
  (Requests / Jobs / Scheduler / System / Infra / Other). Power users on
  unusual stacks may want to define their own regex-based groups in
  Settings. Plumbing is in place: the classifier is one swap away from
  being driven by user rules.

## Ergonomics

- **Command palette** (Ctrl-Shift-P) - search every action, pattern, file,
  rule.
- **Keymap customisation** with a Vim-style preset.
- **Export selection** as plaintext / JSON / markdown for pasting into
  tickets.
- **Copy-as-structured** - copy a record as JSON with parsed fields.

## Reach

- **WSL companion daemon** is on the v1.1 list in `design.md §17`. The
  moment it lands, "open file inside WSL" becomes seamless and unlocks the
  Play 1.x dev environment most users actually have.
- **SSH / SFTP tail** via the same `LineSource` slot - rarer but a
  "kill the competition" feature.
- **Read `.gz` / `.zip` rolled logs** transparently. Common with
  `TimeBasedTriggeringPolicy`.

## Niceties

- **OS notifications on ERROR while tailing** an unfocused tab (cheap,
  behind a per-file toggle).
- **Auto-pause tail on scroll up, resume on jump-to-bottom** - the toggle
  exists; making it automatic-with-override is friendlier.

## Distribution

- **Auto-update via `tauri-plugin-updater`** - design called for this in v1
  (`design.md §16`) but it was deferred out of P10. Design spec:
  [docs/superpowers/specs/2026-05-26-auto-update-design.md](superpowers/specs/2026-05-26-auto-update-design.md).
  Sliced into P11.A (plumbing) and P11.B (signed end-to-end).

## Top picks for v1.1

If forced to pick three, these compound on each other and on the existing
engine:

1. **Minimap heatmap** - visual "where are the errors" at a glance.
2. **Bookmarks + go-to-timestamp** - the navigation primitives that unlock
   investigation workflows.
3. **Merge-by-timestamp across tabs** - the killer feature for anyone
   debugging across multiple logs.

Decide the real v1.1 scope after v1 ships and gets used.
