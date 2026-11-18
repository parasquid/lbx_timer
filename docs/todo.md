# TODO

Planned work only. When an item ships, delete it here and describe the behavior
in the README or the relevant guide.

## Features

- **Keyboard shortcuts.** `1`–`9` start timer N, `Space` pauses/resumes the
  focused timer, `N` adds a timer. They must not fire while Name or Notes has
  focus. Global (unfocused) hotkeys are unreliable on Wayland; app-focused only.
- **Undo an accidental switch** for a few seconds after it happens, without
  rewriting history (a compensating event, or a short grace period before the
  switch is committed).
- **CSV export** with one row per segment (a stretch in which one timer ran):
  session ID and start, timer ID, name, series ID (the piece of work across
  sessions), continued timer ID, segment start and end, duration, what ended
  it, and the timer switched to. The database already records all of these:
  `tasks.series_id`, `tasks.continues_task_id`, `events.cause`, and
  `events.related_task_id`, and `breakdown::segments` derives the segments.
- **Timesheet editing area**, separate from History (for example under
  Settings), for revising the record: rename timers and edit notes after a
  session ended, delete timers or whole sessions, and adjust recorded time.
  It must be clearly marked as destructive, confirm each change, and never
  touch the current session. The engine already supports renaming past timers
  (`update_name_notes`) and deleting them (`delete_history_task`, which removes
  their events and an emptied session, and clears links to them).
- **Idle detection.** While the computer stays awake but there is no keyboard
  or mouse input for a configurable time, ask whether to keep or discard that
  stretch. Suspend is already handled (see README); idle time is not.

## Hubstaff observation

The read-only desktop connection is described in [hubstaff.md](hubstaff.md).
Cloud API access, importing historical intervals, and reconciliation of missed
transitions are outside the current integration scope. Timer control from LBX
to Hubstaff is deliberately excluded.

- **Explicit timing modes in the UI.** Offer **Hubstaff mode** and
  **Independent mode**, with the selected mode clearly visible.
  - In Hubstaff mode, Hubstaff alone determines when a local timer starts or
    pauses. Remove or disable local play/pause actions rather than allowing
    local overrides. Timer selection still chooses which local timer receives
    time, and LBX never sends controls to Hubstaff.
  - Independent mode uses LBX's own play/pause controls without querying
    Hubstaff. The existing `--standalone` launch option provides this behavior;
    expose the choice directly in the product.
  - Define the effect of changing modes during an active session and the
    behavior of End session while Hubstaff continues running. Preserve local
    history and keep the distinction between session completion and tracking
    state explicit. Document and test those decisions before adding the switch.

Further verification:

- Exercise real desktop start/stop transitions and disappearance/reconnection.
- Confirm CLI response compatibility on other Hubstaff desktop releases and
  platforms before extending installation discovery beyond Linux.

## Maintenance

- Upgrade `rusqlite` from 0.32 to the current major version (breaking API
  changes).
- Re-enable antialiasing (`.antialiasing(false)` in `src/app/mod.rs`) once an iced
  release draws multisampled canvas meshes correctly inside scrolled content.
  Check with the manual scroll test in [testing.md](testing.md) on the GPU
  renderer.

## Code quality

Smaller improvements, in no particular order:

- **Storage enforces less than the engine.** `AppData::switch` accepts a timer
  from another session, and `delete_task` accepts a paused timer of the open
  session; only `TimerEngine` prevents both. Move these rules into storage
  (queries or constraints) so the database cannot be misused by a future
  caller, such as the timesheet editing area.
- **One timer per piece of work per session** is assumed by
  `SeriesTotal::sessions` but only enforced by `load_session`. Add a unique
  index on `(session_id, series_id)` at the next schema change.
- **Sleep and recovery wording exists twice**: as the engine's notices and
  in `event_log`. Keep one source.
- **History reloads every session on each tick while a timer runs.** Build
  the current session's entry from the engine's in-memory state instead, so
  the cost does not grow with history.
- **`TimerEngine` mixes the live session with history queries** (`history`,
  `events`, `task`, `series_totals`, `session_segments`). A separate read-only
  query type would keep the engine focused on the current session.
- **Shared test helpers.** Temporary database paths, `at()`, and cleanup are
  written out separately in `src/timer.rs` tests, `tests/lifecycle.rs`, and
  `src/app/tests.rs`.
- **UI tests sleep on the real clock** (tooltip delay, double-click window,
  live history). Injecting time would make them faster and deterministic.
- **Title truncation counts characters**, not rendered width, so wide glyphs
  can still overflow a slot. Switch to width-aware clipping when iced offers
  an ellipsis.
- **Names typed into timers that never ran are dropped by End session**
  without notice.
- **Timer colors follow the timer number**, so a resumed timer can change
  color between sessions. Derive the color from the series instead.
- **Details saves on every action**, even when nothing changed; skip the
  write when name and notes are unchanged.
- **Adding a timer does not clear a stale error** the way other actions do.
- **History view code calls session IDs `key`**; rename to `session_id`.

## Verification gaps

Covered only by reasoning or simulation so far; check on real systems:

- Wayland sessions and GPU (wgpu) rendering.
- A real window manager's close button (the close path is tested through the
  engine and the UI message, not a native close).
- Real suspend/resume (sleep handling is tested with injected clock gaps).
- macOS, Windows, and WSL2.
