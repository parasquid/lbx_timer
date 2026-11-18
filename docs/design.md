# UI design guide

This guide records the LBX Timer UX contract and the implementation constraints
that protect it.

## Product requirements

- The default window (480×720) is narrow and primary: the clock, its controls,
  and a list of up to four linked timers remain visible without scrolling. From
  the fifth timer on, the list scrolls inside a fixed-height slot.
- The layout responds to width. Below 960 px it is a single column that fills
  the window up to 640 px and stays centered beyond that. From 960 px on,
  Details and History move into a second pane beside the clock and timers and
  are always open there; the two panes grow up to 1280 px and stay centered.
- In the single-column layout, Details and History are secondary. They are
  collapsed by default and open only from explicit disclosure controls. Ticks, Start, Pause, Resume, switching,
  End session, errors, and selection do not automatically expand or collapse
  either panel.
- Every launch starts a new session with one unstarted timer. Start begins it
  immediately in standalone mode, without a prompt. Hubstaff mode checks at
  launch and automatically starts the current timer on its first verified
  tracking response, including if Hubstaff was already running. Unnamed timers
  display as `Timer N`. Task
  details are optional and consist of Name and Notes in the UI.
- A session's timers behave as one radio group. Every row has its own square
  play/pause icon button (with a hover label: Start, Resume, or Pause), so any
  timer starts in one click and the running one pauses in the same step.
  Icons are drawn as vector shapes rather than font glyphs, so they look the
  same on every system. Clicking a row body focuses it (clock card and Details)
  without starting it immediately. While Hubstaff is stopped this also chooses
  the timer that will follow its next start; while tracking it is focus-only.
  The running row is
  green and the focused row blue. An unstarted extra timer offers × (remove).
- New timers are always created unstarted. Ending or manually pausing never
  resumes another timer. Hubstaff restarting may resume only the timer it
  previously paused, subject to the cancellation rules below.
- **End session** sits in the clock card's second slot and is enabled once the
  session has recorded time. It is the only way to complete timers; there is no
  per-timer Stop. Whenever the current session changes (End session, or
  resuming with "End current session first"), Details, notices, and pending
  dialogs are reset to the new session's first timer.
- Actions are named by their effect, not their mechanism: **Resume session**
  (in an expanded History entry) continues that session's work; internally it
  creates unstarted timers linked to that session's timers. Session clocks and
  the primary **This session** total show time recorded in that session. When
  any current timer continues earlier work, the Timers header also shows
  **Across sessions**: the sum of the current timers' cumulative totals, with
  each continuation chain counted once and Hubstaff corrections applied.
  Sum the displayed whole-second values so the total agrees with the rows.
  Reserve the secondary line even for a fresh session so loading or ending a
  session never shifts the timer rows; four rows still fit at 480×720.
  The running total of a
  piece of work is secondary information: a small second line in the timer
  row, a suffix on the clock card's state line, a line in Details, and a tag on
  History entries. Resuming asks first only when the current session already
  has recorded time (add to it, end it first, or cancel).
- Details shows an event log, not raw event kinds. Each line is a complete
  sentence naming the timer, the action, the other timer of a switch, and the
  trigger: manual LBX action, Hubstaff startup/recovery, End session, window
  close, sleep, or crash recovery. Automatic pauses include the specific reason.
  Older events without a recorded trigger say so. Preserve fixed event-list
  geometry while allowing longer descriptions to wrap and scroll.
- An unnamed timer is called `Timer N` everywhere: clock card, list, history,
  and as the Name field's placeholder. Typing into an unnamed timer's empty
  Name starts from a clear field. Focusing a Name that has text selects all of
  it, so typing replaces it; a further click places the cursor to edit in place.
- Normal operations do not produce recurring tutorial or success copy. Show
  persistence errors, recovery/sleep uncertainty, and other meaningful
  consequences only when they matter.
- History has one row per session showing the total across its timers.
  Expanding it shows per-timer totals and shares, a proportional bar, and a
  timeline of switches and paused stretches. Colors tie each timer's list row,
  bar segment, and history row together.
- History is a read-only record. It has no rename, edit, or delete controls,
  and Details shows a past timer read-only (name, notes, totals, log). Only the
  current session's timers are editable. Revising recorded time belongs in a
  separate, explicitly destructive editing area (see `todo.md`), never in
  History.
- Expanding a History entry offers **Report** for that session and its entire
  continuation group. The report is an overlay with a combined total and one
  row per continued piece of work, showing its latest name, combined time,
  share, and full notes. Use stored continuation identities, never name matching;
  count each recorded portion once, including branches and explicitly combined
  earlier work. Keep distinct nonempty notes from all included timer records,
  deduplicating carried notes. Include current unstarted continuations and
  their unsaved-in-memory details without creating database records.
- Reports show a timestamped snapshot with **Copy report**, **Refresh**, and
  **Close**. Copy includes the same full notes, totals, and shares as the view.
  Local time continues while the report is open; Refresh updates the snapshot.
  Show an Unverified label in both the report and copied text when included
  time still awaits Hubstaff evidence. Apply confirmed corrections to totals.
  **Recording details** is collapsed initially and reveals the contributing
  saved portions on request. Keep footer actions visible while names, notes,
  and recording details wrap/scroll within the report. The overlay does not
  reflow the clock or change its focus, running state, or disclosure states.

## Hubstaff observation

- The connection is one-way and read-only. Hubstaff stopping pauses LBX and
  preserves its session. LBX controls, End session, sleep, recovery, and close
  never change Hubstaff.
- Launch follows Hubstaff once: show Checking immediately, wait through
  initial stopped/unavailable states, and start the current unstarted local
  timer at the first verified running observation. Never backfill earlier time.
  Local cancellation and failed automatic start consume the startup intent;
  End session never arms the next session. Standalone launch remains idle.
- A confirmed Hubstaff stop arms the ID of the timer that was running;
  the next confirmed tracking observation resumes
  that timer once unless the user selected a different current-session timer
  while Hubstaff was stopped. That selection chooses the next Start/Resume
  target without counting time. While Hubstaff runs, row selection is focus-only.
  Explicit Pause, End session, close, Start/switch, session loading, removing
  the target, or selecting History cancels it. A lost or stale connection
  preserves it until a fresh tracking response permits recovery.
- Explicit Start/Resume and switching require a fresh tracking check. Automatic
  resume uses the fresh running observation and never backdates elapsed time.
- In Hubstaff mode, show a compact, stable indicator for checking, tracking,
  stopped, and unavailable states. Standalone keeps the existing title bar.
  Include the selected Hubstaff project as context
  without allowing a long name to move or overflow adjacent controls.
- Disable clock and row Start/Resume actions while tracking is unverified or
  stopped. Keep local Pause and End session available. Enforce the same rule
  in message handling, including stale responses and pending-start cancellation.
- On connection failure or stale status, keep the active timer counting and
  mark it **Unverified** in the clock state and connection badge. Save the
  interval from the last verified observation. After recovery, automatically
  confirm it against Hubstaff's local tracking audit or exclude any stopped
  portion from displayed totals and the timeline. Clear the Unverified label
  after confirmation or adjustment; Details records either outcome. Missing
  evidence remains Unverified and retries automatically. Saved History also
  labels unresolved time. Original local timing remains available.
- Local Pause, End session, close, and crash recovery preserve pending
  verification. Confirmed Hubstaff stops and system sleep/clock-jump pauses
  still pause timing. Never silently fall back to standalone mode. If saving
  uncertainty fails, report the storage error and fall back to a durable pause.
- Four timer rows and their controls still fit at 480×720. Connection changes
  must not reflow the clock or expand Details/History. The full contract and
  polling limitations are in [hubstaff.md](hubstaff.md).

## Visual and implementation constraints

- Use a common timer-card and control structure for idle, running, paused,
  end session, adding timers, normal ticks, notices, and long names. Reserve
  stable text/control slots so changing state does not move neighboring
  controls.
- Render the clock with a tabular/monospaced font. Use responsive `Fill`, fixed
  minimum slots, and scrollables for content; do not use absolute-positioned
  fake UI or screenshot-specific coordinates.
- Give persistent scrollables and repeated rows stable widget identity where the
  framework supports it. A modal confirmation should overlay the base layout so
  opening it does not permanently reflow the timer.

## Verification

`iced_test::Simulator` tests should exercise real widget interactions and assert
widget bounds at the default viewport across idle/running/paused/end session,
adding timers, ordinary ticks, long names, and notice/error states. They should
verify one-click row switching, the four-row list limit, disclosure flows, the
Name field's placeholder and select-on-focus behavior, both responsive layouts,
the history breakdown, the read-only record, and resuming a session.
Persistence tests use disposable databases and verify event cascade,
unrelated-row retention, and current-session protection.

## Rationale sources

These official framework and platform guidance pages inform the rules above:

- [iced simulator](https://docs.iced.rs/iced_test/simulator/struct.Simulator.html),
  [keyed widgets](https://docs.rs/iced/latest/iced/widget/keyed/index.html),
  and [iced fonts](https://docs.rs/iced/latest/iced/struct.Font.html) support
  widget-level interaction/layout checks, stable repeated content, and the
  monospaced clock choice.
- [Apple writing guidance](https://developer.apple.com/design/human-interface-guidelines/writing),
  [Apple alerts](https://developer.apple.com/design/human-interface-guidelines/alerts),
  [GNOME HIG principles](https://developer.gnome.org/hig/principles), and
  [Microsoft commanding basics](https://learn.microsoft.com/en-us/windows/apps/design/basics/commanding-basics)
  support concise action labels, progressive disclosure, and explicit
  consequence-aware confirmation for destructive actions.
- [Material typography](https://m3.material.io/styles/typography/applying-type)
  documents the use of tabular/monospaced figures where numeric alignment
  matters.
