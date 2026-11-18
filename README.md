# LBX Timer

A native, Linux-first Rust work timer for Labelbox, built with iced and SQLite.

## Run

From the LBX Timer project root:

```bash
cargo run
```

The first build may take longer while Cargo downloads and compiles dependencies.

Every launch starts a new **session** holding one timer at `00:00:00`. In
Hubstaff mode it starts automatically when Hubstaff is confirmed tracking,
including when Hubstaff was already running at launch. In standalone mode,
click **Start** to begin. Adding a name or notes is optional. Closing the window pauses
the running timer and keeps the session in history. If the app crashes, the
running timer keeps the time up to its last save, and nothing is added for the
time the app was not running.

## Linked timers

A session's timers are linked like a radio group: at most one runs at a time.

- Every row has its own play/pause button (hover it for its label: Start,
  Resume, or Pause). Starting any timer pauses whichever timer was running, in
  a single save with a single timestamp, so no time is lost or counted twice.
- **+ Add timer** adds an unstarted timer at `00:00:00` and never affects the
  running timer. While Hubstaff is stopped, selecting that new timer chooses
  it to start when tracking is next confirmed.
- Click a row to show that timer in the clock card and in **Details**.
- **×** removes a timer that has never run. Timers with recorded time stay
  until the session ends.
- **End session** completes every timer and starts a new session with one timer.
  Details then shows that fresh timer, so nothing from the ended session is
  left on screen. Name timers and add notes during the session: once it ends,
  its timers are part of the record.

The **Session** total beside the list is the sum of all timers. The running
timer's row is green; the row shown in the clock card is blue.

Unnamed timers are called `Timer 1`, `Timer 2`, and so on; the Name field in
Details shows that name until you type your own. Focusing a Name that you have
already typed selects all of it, so typing replaces it; click again to place the
cursor and edit it in place.

## Hubstaff

When the installed Hubstaff desktop CLI is detected, LBX observes it read-only.
Start/Resume is available while Hubstaff is tracking. Stopping Hubstaff pauses
LBX; starting Hubstaff again resumes that same timer unless a local action
canceled it. Select another current-session timer while Hubstaff is stopped to
make it the one that starts/resumes next. Launch follows Hubstaff from the first confirmed running response;
manual pauses and ended sessions stay idle.
**Pause, End session, switching timers, and closing LBX never change Hubstaff.**

The connection needs no API credentials. Its indicator shows tracking,
stopped, checking, or unavailable. If a connection fails while a timer is
running, LBX keeps counting and marks that time **Unverified**. After connection
returns, it automatically checks Hubstaff's local tracking audit: supported
time is confirmed and any stopped portion is excluded from totals and the
timeline. The Unverified label clears after successful reconciliation. Original
local timing and the confirmation/correction remain in the saved record.
Missing or incomplete evidence stays Unverified and is retried automatically.
New starts and switches still require fresh tracking confirmation. Details shows whether
each new event was triggered manually or by Hubstaff, including the reason for
automatic pauses; older events without a recorded trigger are labeled. Polling can
miss brief transitions, so LBX totals are local measurements, not an exact
copy of Hubstaff's recorded time.

Use `cargo run -- --standalone` for independent local timing. Setup, command
restrictions, and timing limits are documented in
[Hubstaff observation](docs/hubstaff.md). Use the
[controllable mock](docs/hubstaff-mock.md) to test without your real tracker.

## Window layout

In windows narrower than 960 pixels, the app is one centered column with
Details and History behind disclosures. Wider windows show Details and History
in a second pane beside the timers. The layout and interaction rules are in the
[UI design guide](docs/design.md).

## History

History lists one entry per session with the total across all its timers.
Expanding an entry shows each timer's total and share of the session, a bar in
the timers' colors, and a timeline of the switches, including paused stretches.
Select a timer there to view it in **Details**: its name, notes, totals, and
event log.

History is a read-only record. Nothing in it can be renamed, edited, or deleted;
starting new work from it (below) adds to the record and never changes what is
already there.

The event log reads as a plain record of what happened. Every line names the
timer, the action, the other timer in a switch, and the reason for automatic
pauses, for example `Paused “Review” — switched to “Email”`. Each line also
shows when it happened and the timer's time in the session afterwards.

## Resuming a session

**Resume session** in an expanded History entry picks that session's work back
up, so yesterday's three timers can be continued today:

- Its timers come back into the current session with their names and notes.
  Each starts at `00:00:00` for the current session and is linked to the timer
  it continues; the earlier session itself is not changed.
- They replace the current session's untouched placeholder timer. If the
  current session already has recorded time, the app asks whether to add them
  to it or to end it first. A session holds at most nine timers, and work that
  is already in the current session is not added twice.
- A continued timer shows its running total, the time across every session of
  that piece of work, under its session time in the list ("02:11:05 total"), on
  the clock card ("RUNNING · 02:11:05 in total"), and in Details.
- The Timers header shows **This session** and, for resumed work,
  **Across sessions**. Across sessions sums the cumulative totals of all
  current timers, counting each continuation chain once and applying Hubstaff
  corrections. New timers added to the set contribute their current time.
- In History, a session that continues earlier work is tagged "continues Oct 09"
  and one that was picked up later "continued later". Expanded entries link to
  the related sessions.

Each session's own totals still only count time recorded in that session, so
session totals never double count.

Stable task IDs keep start, pause, resume, and stop events connected when a task
is renamed. Events store UTC timestamps; the UI displays local times.

## Session reports

Open **History**, expand a session, and choose **Report**. The report combines
the original session and all continuations into one total and timer breakdown,
including timer names, full notes, combined durations, and percentages. It
counts each recorded portion once and uses Hubstaff-corrected time. Timers are
grouped by their continuation identity; unrelated timers with the same name
stay separate. Explicitly combining earlier work also joins those recordings
in the report.

Each timer uses its latest name and keeps distinct nonempty notes from its
included records. Notes copied into later continuations appear once; changed
notes remain separate paragraphs. Current unstarted continuations and their
in-memory details are included without being written to storage.

**Copy report** copies the displayed summary and notes as plain text. The
report is a timestamped snapshot; **Refresh** includes subsequent time and
changes, and **Close** returns to History. The running timer continues while
the report is open. Remaining Unverified time is identified in both the view
and copied text. Expand **Recording details** for the contributing saved
portions. Reporting is read-only and requires no database migration.

## Sleep and suspend

Timers count time from the system's monotonic clock, which on Linux
(`CLOCK_MONOTONIC`) and macOS stops while the computer is suspended. So
suspended time is never added to a timer.

On every tick (about twice a second), and before every action, the app also
compares that clock with the wall clock, which keeps going during suspend. If
the wall clock has moved more than five seconds further, the running timer is
paused and marked `sleep_pause`, and a notice explains what happened. Nothing
resumes on its own; press Resume when you are back.

A forward jump of the wall clock, such as a manual clock change, looks the same
and is handled the same way. Backward jumps are ignored.

The app cannot tell when you step away while the computer stays awake; the
running timer keeps counting. Other platforms do not guarantee that their
monotonic clock stops during sleep, so this only holds on Linux and macOS.

## Local data

The default database is `$XDG_DATA_HOME/lbx-timer/timer.sqlite3`, falling back
to `$HOME/.local/share/lbx-timer/timer.sqlite3` on Linux. Back up the database
with the app closed. A per-database lock prevents two app instances from using
the same database.

When data is saved:

- Every start, pause, switch, and End session is written immediately, in one
  SQLite transaction, before the screen changes.
- While a timer runs, its elapsed time is checkpointed about every half second
  (on each UI tick). After a crash, the next launch records that timer as
  paused at its last checkpoint: at most the last half second before the crash
  is lost, and nothing is added for the time the app was not running.
- Name and Notes are saved on every keystroke for a timer that has run. For a
  timer that has never run, they are kept in memory and written when it first
  starts; nothing is stored for a session until a timer starts.
- Closing the window pauses the running timer and closes the session.

SQLite runs in WAL mode with `synchronous = FULL`, so a committed write survives
a crash or power loss.

Supported older databases upgrade automatically in a transaction. Schema 5
upgrades to schema 6 by adding verification metadata; existing sessions,
timers, events, names, notes, links, and local recorded time are preserved.
Compatibility with previously supported releases is required for future
changes. Unknown future schemas and older unsupported experimental layouts
are checked read-only and refused without modifying their contents.

Advanced toolchain, build, test, and custom-database instructions are in
[docs/development.md](docs/development.md).

## Future work

Planned features are tracked in
[docs/todo.md](docs/todo.md).
