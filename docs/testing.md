# Testing

## Running the checks

```bash
ICED_TEST_BACKEND=tiny-skia cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s scripts -p 'test_mock_hubstaff.py'
```

Run them on both the minimum supported Rust (`rust-version` in `Cargo.toml`,
for example `cargo +1.89 test ...`) and current stable. `--locked` also proves
that `Cargo.lock` resolves as committed. Every test uses its own disposable
database; none touch the user's data.

## What the suites cover

**Library (`src/`)**

- `report`: aggregate original sessions, chains, branches, and explicitly
  combined work; count each task once; group by series rather than name;
  retain renamed timers and distinct multiline notes; exclude unrelated work.

- `provider`: read-only CLI command selection, JSON validation, readiness,
  discovery, bounded subprocess failures, and status freshness. Fake executables
  provide responses without contacting or controlling the real desktop app.

- `breakdown`: building segments from events, idle gaps in the timeline, and
  placing a crash-recovered segment at its checkpoint rather than at the next
  launch.
- `event_log`: the wording of every event kind and cause, including a switch
  partner that was deleted.
- `app::format` (in the binary): durations, totals and percentages that add
  up, title truncation, and share-bar weights.
- `timer`:
  - Radio-group switching and pause/resume accounting.
  - Sleep-gap pausing and crash recovery.
  - New session per launch; unstarted timers that keep their details in
    memory; the add/remove rules.
  - A randomized property test: over many switch/pause/tick sequences,
    segment sums equal each timer's total, and timer totals add up to the
    session total.
  - Resuming a session (`load_session`): links, running totals, both modes,
    rejection without changes (duplicates, the current session, too many
    timers), and deleting the timer a resumed timer continues.
  - Event logs that name both timers of a switch and the continued timer.

**Lifecycle (`tests/lifecycle.rs`)**, through the public API and real SQLite
files:

- **Atomicity:** injected storage failures during a switch and during End
  session leave every row unchanged.
- **Sleep and crashes:** a sleep gap detected while switching pauses only the
  running timer. A crashed linked session is closed as `recovered` at its
  last checkpoint.
- **Nothing by default:** a launch without recorded time writes nothing.
- **Links:** switch events record the other timer, and deleting it clears
  the link without removing events.
- **Storage rules:** the one-running-task and one-open-session rules,
  upgrades from supported schemas, refusal of unknown schemas, the file lock, deletion
  cascades, and current-session protection.
- **Hubstaff pauses:** external pause reasons persist through reopening without
  changing the database schema or ending the local session.

**UI (`src/app/tests.rs`)**, with `iced_test::Simulator` driving real widget
interactions:

- **Aggregate reports:** open from History; complete timer notes, totals,
  percentages, copy text, collapsed recording details, and closing without
  changing focus/session. Refresh updates a snapshot while local timing
  continues. Unverified time is identified. Dialog and footer geometry fits
  narrow, default, and wide windows; long report content scrolls internally.

- **Timer rows:** one-click start from any row, including while another
  timer is focused. The row buttons are square icons.
- **Layout stability:** clock and control geometry stays fixed across every
  state and as timers are added. At the default size, four timers fit with
  every control on screen, and more scroll.
- **Name field:** its placeholder, select-on-focus, and click-to-edit
  behavior.
- **Responsive layouts:** the single-column and two-pane layouts, including
  centering in very wide windows.
- **History:** the breakdown (totals, shares, timeline), displayed totals
  that add up, and the read-only record: no delete or rename controls, past
  timers shown read-only in Details, and edits to them ignored.
- **Resuming work:** Resume session with and without recorded time (add, end
  first, cancel), running totals in rows, the clock card, and Details,
  history tags, and links between sessions.
- **Cleanup:** End session resets Details, notices, and errors to the new
  session.

**Hubstaff UI and policy**

- Start/Resume and row switches require a fresh running observation. Direct
  messages enforce the same rule as disabled widgets.
- Healthy polling runs every two seconds and skips cached readiness. Failed
  attempts retry after 2/4/8/16/30 seconds, reset on success, and never overlap.
  Explicit starts bypass retry delays. The six-second freshness window allows
  a normal interval plus a slow query. Fake executables verify command selection
  and bounded errors for both readiness/status batches and status-only checks.
- Confirmed stopped observations pause only the local timer and preserve the
  session. Unavailable/stale observations keep it counting as Unverified;
  repeated failures do not duplicate intervals or add pause/resume events.
- A confirmed stopped-to-tracking cycle resumes the timer that Hubstaff
  paused, once. Selecting another current-session timer while Hubstaff is
  stopped makes that paused or unstarted timer the next target instead.
  History selection never arms a target. Hubstaff startup follows its first verified running response,
  while standalone startup stays idle. Connection failures and stale status
  preserve locally counted time and automatically verify against the desktop
  audit on recovery. Explicit local cancellation and system sleep prevent automatic
  recovery. Pause and End session cancel pending starts;
  late responses cannot resurrect a timer from a finished session.
- Connection indicators, disabled controls, and long project names preserve
  compact-window geometry. Local End session remains usable while Hubstaff is
  running, stopped, or unavailable.
- Details displays persisted manual and Hubstaff event triggers while retaining
  fixed panel geometry. Reopening preserves triggers and old events with no
  trigger. Audit coverage, missing evidence, hidden stop/restart transitions,
  exactly-once deductions, timeline/total consistency, and continuation totals
  are covered. Schema 5 migration preserves records and schema 6 reopens.

## Review snapshots

UI tests write Tiny-Skia renders to `target/ui-snapshots/`: idle, four timers,
switched, row-button hover label, history, breakdown, resumed session, resume
confirmation, and wide. They are review artifacts, not committed baselines. A
test fails if a render differs from the file already there, so delete the
directory after an intentional UI change and look at the new images. The
simulator does not search tooltip overlays, so hover labels are checked only in
the hover snapshot. The durable regression guard is the simulator's interaction
and geometry assertions.

## Manual smoke test

For changes to windowing, input, persistence, or recovery, run the real binary
against a disposable database. Without a desktop session, use Xvfb, a window
manager, and `xdotool`:

```bash
Xvfb :77 -screen 0 1600x1000x24 &
export DISPLAY=:77 XDG_RUNTIME_DIR=$(mktemp -d)
mutter --x11 --replace --sm-disable &   # any WM; without one, keys never arrive
lbx_smoke_dir=$(mktemp -d)
cargo run -- --standalone --db "$lbx_smoke_dir/timer.sqlite3" &
```

`xdotool search --name "LBX Timer"` also matches the window manager's frame.
Send clicks to the app's own (client) window so that coordinates match the
layout.

Then check, with `sqlite3` on the disposable database in `$lbx_smoke_dir`:

1. Left idle, a fresh launch writes no tasks, events, or sessions.
2. Start a timer, add a second, and start the second from its row. The first
   timer's `pause` and the second's `start` (both with cause `switch`) share one
   timestamp.
3. `kill -9` the app and relaunch. The running timer gets `recovery_pause` at its
   last checkpoint, the session is closed as `recovered` at that checkpoint, and
   the app shows the recovery notice and a fresh session.
4. In Details, click Name and type into an unnamed timer; then click away,
   click Name once, and type: the name is replaced. Click away, click Name
   twice (a moment apart), and type: the text is inserted at the cursor.
5. `xdotool windowsize` the window past 960 px. Details and History move into
   the second pane and keep updating, and wider windows keep the content
   centered.
6. With History (and a session) expanded and more than four timers, scroll the
   window and the timer list up and down (`xdotool click 4` / `click 5`). The
   row icons move with their rows, with no stale or duplicated icons.

The simulator always renders with the CPU renderer (Tiny-Skia), while a normal
launch uses the GPU renderer (wgpu) when one is available, so GPU-only drawing
bugs need this manual pass. Force a renderer with `ICED_BACKEND=wgpu` or
`ICED_BACKEND=tiny-skia` to compare the two.

Gaps that still need real systems are listed in [todo.md](todo.md).

## Mock Hubstaff

Use the [mock setup and transition checklist](hubstaff-mock.md) to test the
connection without your real Hubstaff timer. The mock state file and disposable
LBX database are separate from production data. The real CLI response evidence
and simulated cases are listed there explicitly.

## Hubstaff desktop smoke test

Use a disposable database while preserving the normal home directory so CLI
installation discovery still works:

```bash
cargo run -- --db /tmp/lbx-hubstaff-smoke.sqlite3
```

Start and stop Hubstaff manually in its own desktop app. LBX must never send a
Hubstaff control command during these checks.

1. With Hubstaff stopped, confirm LBX shows stopped, its Start/Resume controls
   are disabled, and no local timer starts.
2. Start Hubstaff manually. Confirm LBX shows tracking and the selected project,
   and starts its current local timer automatically at confirmation, from zero.
   Also launch LBX while Hubstaff is already tracking and verify the same result.
3. Add a second LBX timer and switch to it. Confirm only one local
   timer runs and Hubstaff tracks continuously.
4. Stop Hubstaff manually. Confirm LBX pauses after the next successful check,
   keeps its session and totals, and records the external pause reason. Leave
   it stopped for several checks and confirm no duplicate pause events appear.
5. Start Hubstaff again. Confirm the same local timer resumes once. Stop it,
   select another current-session timer in LBX, and start Hubstaff: the selected
   timer should start/resume while the previous one stays paused. Confirm the
   session ID is unchanged. Pause locally, then stop/start Hubstaff without a
   new selection: LBX must stay paused.
6. While both are running, Pause in LBX. Confirm Hubstaff keeps running.
   Resume LBX, then End session. Confirm Hubstaff still runs and LBX presents a
   new unstarted timer. Start that local timer to begin the next session.
7. Make the CLI temporarily unreachable while Hubstaff keeps tracking. Confirm
   LBX keeps counting as Unverified and blocks new starts. Restore access:
   the audit check should confirm the interval and clear Unverified. Repeat
   with a stop/restart during the outage; only unsupported time should be
   excluded automatically. Repeat with a local Pause during the outage:
   recovery must leave it paused. Inspect Details for verification and manual
   triggers. Do not substitute cloud network loss for local CLI failure:
   a logged-in, ready desktop tracker may work offline.
8. Close LBX while Hubstaff runs. Confirm Hubstaff remains running. Reopen LBX
   on the disposable database: a fresh local session should start following the
   verified running state. Inspect the older saved session and pause reason.

Measure response delay from the manual stop to LBX's pause. Polling observes
state at query time and does not establish the exact external stop timestamp.
A quick stop/start between healthy checks may be missed; automatic audit
reconciliation covers saved Unverified intervals. Record the CLI version and whether
live checks passed separately from automated test results.
