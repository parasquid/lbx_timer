# Hubstaff observation

LBX observes the local Hubstaff desktop app. The connection is one-way:
Hubstaff can pause and resume local timing, and every LBX action affects only LBX.
The adapter uses the installed desktop CLI and the desktop app's existing
login. No API credentials, cloud API requests, webhook subscriptions, or
manual time uploads are needed.

## Behavior

| Action or condition | LBX result | Hubstaff result |
| --- | --- | --- |
| Launch while Hubstaff is running | New session; current timer starts at the first verified running response | Unchanged |
| Launch while Hubstaff is stopped | Wait; start the current timer when tracking is first confirmed | Unchanged |
| Start or resume an LBX timer | Check Hubstaff; start locally only if tracking | Unchanged |
| Switch LBX timers | Check Hubstaff; pause the previous local timer and start the selected one | Unchanged |
| Hubstaff stops | Pause the running LBX timer; preserve its session | Unchanged |
| Select a current-session timer while Hubstaff is stopped | Choose that timer for the next confirmed start; do not count yet | Unchanged |
| Hubstaff starts again | Start/resume the chosen timer once, unless canceled locally | Unchanged |
| Pause in LBX | Pause the local timer | Keeps its current state |
| End session in LBX | Save the session and prepare a new one | Keeps its current state |
| Close LBX | Save and close the local session | Keeps its current state |
| Hubstaff cannot be verified | Keep the active timer counting as Unverified; block new starts/switches; automatically reconcile after recovery | Unchanged |

Only one local timer runs at a time. Hubstaff tracks the overall work period;
LBX divides the locally recorded portions into timers and
sessions. Their totals need not match: Hubstaff may run while LBX is paused,
closed, or between sessions.

The selected Hubstaff project is displayed for context. This version observes
whether the desktop tracker is running on any project; it does not enforce a
project mapping or change Hubstaff's selected project. Local names and notes
are never sent to Hubstaff.

## Automatic resume

A confirmed Hubstaff stop remembers the ID of the local timer that was
actually running, pauses it, and preserves its session. A fresh tracking
observation resumes that same timer at observation time. If the user selects
another timer in the current session while Hubstaff is confirmed stopped, that
timer becomes the next target instead. An unstarted target starts; a paused
target resumes. Selection itself records no time. Repeated running responses do
not add duplicate Start/Resume events, and the observed stopped interval is
excluded.

The resume target is an in-memory ID in the current session, not the name of a
timer or whichever History entry happens to be selected. Explicit local intent
cancels it: Pause, End session, close, explicit Start/switch, loading another
session, removing the target, or selecting a History record. Selecting a current
session timer while Hubstaff is stopped explicitly chooses a new automatic
target. Selecting a row while Hubstaff is running only focuses it and does not
switch the running timer. Unknown/stale status cannot authorize selecting a new
target, but preserves the timer paused by Hubstaff observation.
Restarting Hubstaff must not resurrect a timer from an ended session.

Startup follows Hubstaff once for the new local session. The badge immediately
shows Checking, then Tracking, Stopped, or Unavailable. A fresh tracking response
starts the current unstarted local timer at observation time. An initial stopped
response leaves LBX waiting for Hubstaff to start; pre-start readiness failures
do not count time, and startup can still follow a later verified running state.
Explicit local cancellation or a failed automatic start consumes that startup
intent. It must never start a new session after a local End session request.
Standalone mode still waits for a local Start.

Unavailable, stale, or malformed status keeps an already running timer counting
and saves an Unverified interval from its last confirmed running checkpoint.
Repeated failures extend the same interval. A successful tracking response
closes the interval without adding pause/resume events. A successful stopped
response pauses the timer and closes the interval. Local Pause, End session,
close, and crash recovery also close and preserve the interval. System sleep
or a clock-jump pause still requires explicit resume.
If saving an external pause fails, that pause must be committed at its original
boundary before any automatic resume is permitted by a subsequent fresh poll.
If saving an automatic resume fails, report it and require a manual retry.

Details records the trigger on new Start, Resume, Pause, and switch events:
manual LBX actions, Hubstaff startup, or Hubstaff tracking returning after an
automatic pause. Hubstaff pause events retain the stopped/timeout/stale reason.
End session, window close, sleep, and crash recovery identify their own triggers.
The existing event details column stores this information without a schema
change. Older events without a recorded trigger are labeled as such; their
origin is not inferred from timestamps or adjacent events.

## Automatic verification and corrections

After a fresh observation, LBX checks closed Unverified intervals against the
local desktop audit log off the GUI thread. A current Tracking response alone
does not prove that Hubstaff tracked throughout the gap. Successful verification
marks the time Confirmed. If the log shows stopped stretches, LBX marks the
interval Adjusted and excludes those stretches automatically from the live
clock, session/history totals, continued totals, and timeline. The Unverified
indicator clears for both outcomes. Details records the interval, query failure,
and evidence or correction. There is no manual verification step.

The original local elapsed values and events remain stored. Separate exclusion
metadata supplies corrected totals and splits the original timeline. Applying
the same result again cannot deduct time twice. Pause/end/close and relaunch
do not discard pending verification; old sessions can reconcile in the background.

The default evidence path is `$XDG_DATA_HOME/Hubstaff/logs/audit.log`, falling
back to `$HOME/.local/share/Hubstaff/logs/audit.log`. Override it with
`LBX_HUBSTAFF_AUDIT_LOG` for another installation or an isolated test log.
LBX reads only this local file, up to 8 MiB, and never changes Hubstaff.
The parser uses observed desktop START_TRACKING/STOP_TRACKING records and
timezone-qualified timestamps. Missing/truncated logs, no preceding tracking
anchor, a desktop restart in the interval, reversed timestamps, a clock-boundary
discrepancy over one second, or audit/status disagreement leave time Unverified.
Retries occur automatically about every five seconds while fresh status is
available. Log rotation that removes the necessary anchor can prevent automatic
confirmation. The UI never labels missing evidence as confirmed.

Schema 6 adds verification metadata through an automatic transactional upgrade
from schema 5. Existing rows and local time are preserved. Supported migrations
must remain available in future releases.

## Setup

Normal launch discovers `HubstaffCLI.bin.x86_64` in the user's `Hubstaff`
installation directory or on `PATH`:

```bash
cargo run
```

For another installation location, set the executable path explicitly. This
example path is a placeholder; replace it with the actual executable:

```bash
LBX_HUBSTAFF_CLI=/path/to/HubstaffCLI.bin.x86_64 cargo run
```

An invalid explicit path is a connection error, not evidence that Hubstaff is
absent. Correct the path and restart LBX to retry installation discovery.
When an installation is found but unavailable, LBX remains in Hubstaff mode and blocks starts until a fresh tracking response is available.
It never silently switches to independent local timing.

When no installation is found, LBX uses standalone local timing. To choose
standalone timing explicitly even with Hubstaff installed:

```bash
cargo run -- --standalone
```

Standalone mode does not query Hubstaff. Either mode supports the existing
`--db` argument for a disposable or custom database.

## Read-only boundary

The adapter invokes only these fixed command arguments, without a shell:

```text
HubstaffCLI.bin.x86_64 send wait-login
HubstaffCLI.bin.x86_64 send status
```

`wait-login` reports login and readiness. LBX runs it at startup and again after
a failed or stale observation; successful routine checks use only `status`.
`status` reports tracking state and
may include the selected project and its daily tracked total. An offline cloud
connection does not by itself invalidate a ready, logged-in desktop tracker.

The adapter never invokes `start-project`, `start-task`, `resume`, `stop`,
`quit`, deep links, `launch`, or `--autostart`. These restrictions apply to
normal actions, errors, sleep, recovery, and shutdown. Hubstaff must be opened
and controlled by the user.

## Timing and failure handling

Healthy status is polled about every two seconds, including while LBX is idle.
Readiness is cached after a successful check, so normal polling launches just
one local status command. A failed response or stale observation invalidates
readiness; recovery checks login/readiness before querying status again.
Queries run asynchronously with a two-second timeout for the complete batch
and one outstanding batch. Observations expire after six seconds without a
successful response, allowing for the polling interval and a slow query.

After consecutive failures, background retry delays are 2, 4, 8, 16, then
30 seconds, measured from completion of the failed attempt and capped at
30 seconds. Any successful response resets the failure count and restores
the two-second schedule. Explicit Start/Resume and switching request a fresh
check immediately, bypassing the retry delay while still allowing only one
outstanding batch. If another check is already pending, the explicit request
gets its own fresh check as soon as the earlier one finishes.
A delayed response cannot start a
timer after the user has canceled that intent with Pause or End session.

A stopped response pauses LBX at detection time. Repeated stopped responses do
not append duplicate pause events. The reason is retained in the local event
log. Malformed output, a failed command, missing login/readiness, or stale
observations mark active local time Unverified and block new starts/switches.
The active timer keeps counting while the observer retries. If saving the
Unverified metadata fails, LBX reports the error and falls back to a durable
pause instead of counting time without its verification record.
If saving the pause fails, LBX reports the storage error and retries at the
original detection boundary. Retries must not add time after that boundary,
and other timer actions must wait for the pause to be saved. It must not claim
that the pause was saved successfully while persistence is failing.

This is observation-based local timing, not exact synchronization with
Hubstaff's recorded intervals. Detection trails a stop by the polling interval
plus command and scheduling latency. Brief stop/start transitions between
observations can be missed. Outages are checked against the desktop audit and
corrected only when that evidence covers the recorded interval. Ordinary
polling latency outside an Unverified interval is not retroactively corrected.

The CLI's `tracked_today` is a project daily total, not the current interval's
elapsed time. LBX never substitutes it for a local clock or imports time from
before its own timer started. Launching while Hubstaff is already running
starts at zero locally when tracking is first confirmed.

## Verification

Automated checks use fake CLI executables and disposable databases. They cover
read-only argument selection, response validation, failed and timed-out
queries, observation freshness, pause persistence, cancellation of pending
starts, and compact UI geometry. See [testing.md](testing.md) for the suite
commands and manual desktop checks.

For repeatable desktop testing without the real tracker, use the
[controllable mock CLI](hubstaff-mock.md). Its documentation distinguishes
observed real response fields from simulated failures.

Live start/stop behavior must also be verified with the installed desktop app;
passing fake-CLI tests alone does not establish compatibility with every
Hubstaff version. The installed CLI's `--help` and observed JSON responses are
the local interface reference. Cloud and deep-link documentation is retained
separately in [hubstaff-reference.md](hubstaff-reference.md).
