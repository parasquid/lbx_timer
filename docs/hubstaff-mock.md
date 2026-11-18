# Mock Hubstaff desktop CLI

Use the mock to exercise LBX's Hubstaff connection without starting, stopping,
or querying the real Hubstaff app. It implements the two read commands used by
the adapter and has a separate controller for changing the simulated state.
It requires Python 3 and uses no third-party packages or network access.

## Run with LBX

From the project root, create an isolated state directory and explicitly select
the mock executable:

```bash
lbx_mock_dir=$(mktemp -d)
export LBX_MOCK_HUBSTAFF_STATE="$lbx_mock_dir/state.json"
export LBX_HUBSTAFF_CLI="$PWD/scripts/mock-hubstaff.py"
scripts/mock-hubstaff.py set stopped
cargo run -- --db "$lbx_mock_dir/timer.sqlite3"
```

In a second terminal, set `LBX_MOCK_HUBSTAFF_STATE` to the same path printed by
`echo "$LBX_MOCK_HUBSTAFF_STATE"` in the first terminal. The path must match so
the controller and LBX read the same mock state. For example, use this command
with the actual path substituted for the placeholder:

```bash
export LBX_MOCK_HUBSTAFF_STATE=/actual/path/to/state.json
```

Then simulate transitions from the project root:

```bash
scripts/mock-hubstaff.py set running
scripts/mock-hubstaff.py set stopped
scripts/mock-hubstaff.py set running
scripts/mock-hubstaff.py set unavailable
scripts/mock-hubstaff.py set logged-out
```

The fake project is labeled **Mock Hubstaff project**. If LBX shows your real
project, close that LBX instance and check that `LBX_HUBSTAFF_CLI` was exported
in the terminal used to launch it. The real Hubstaff app can remain closed.
Changing the mock state never controls the real app.

## Test sequence

1. Launch the mock stopped. LBX must stay idle with Start/Resume disabled.
2. Set running. LBX automatically starts its current timer. No initial Start
   click is needed. Repeated running reads must not add another Start event.
3. Set stopped and wait for LBX to pause. Set running; the same timer resumes
   once, excluding the observed stopped interval. Stop again, add/select Timer 2
   in the current session, and set running. Timer 2 should start/resume while
   Timer 1 remains paused in the same session.
4. While running, click Pause in LBX. Set stopped, then running. LBX must
   remain paused because the user explicitly paused it.
5. Resume locally, then set stopped. End session in LBX and set running.
   The new session must stay idle; the old timer must not be resurrected.
6. Start the new local timer, then set unavailable. LBX keeps counting with an
   Unverified state. Set running: it continues without a pause/resume event.
   To test automatic confirmation or correction, set `LBX_HUBSTAFF_AUDIT_LOG`
   to an isolated audit log with timestamped START_TRACKING/STOP_TRACKING
   records covering the interval. Without evidence it stays Unverified.
   Repeat with a local Pause during the outage; recovery must leave it paused.
   Inspect Details for verification/correction and manual action triggers.
7. Start a timer and End session while the mock stays running. Its state
   stays running and the new LBX session stays idle.

Leave each mock state active long enough to be observed. A stop/start between
polls can be missed, just as with the real app.

## CLI contract

```text
scripts/mock-hubstaff.py send wait-login
scripts/mock-hubstaff.py send status
scripts/mock-hubstaff.py set running|stopped|unavailable|logged-out
scripts/mock-hubstaff.py --help
```

`send` commands only read the mock state. A missing state file means stopped;
reads must not create it. `set` commands write only the configured mock state
file. The environment variable is required so testing cannot accidentally use
an implicit shared state location. Invalid or unreadable state must fail rather
than silently report tracking. The mock does not implement Hubstaff's timer
control commands such as `start-project`, `resume`, or `stop`.

## Automated checks

```bash
python3 -m unittest discover -s scripts -p 'test_mock_hubstaff.py'
ICED_TEST_BACKEND=tiny-skia cargo test --locked --all-targets
```

The Python tests exercise the controller and read-only commands with isolated
state paths. Rust tests exercise the adapter and LBX transition policy. These
checks do not need a real Hubstaff installation or a running desktop tracker.

## Evidence and simulation boundaries

The installed Linux CLI, version **1.9.8-92139457**, supplied the observed
response structure below. Both `tracking: false` and `tracking: true` have been
observed through real read-only commands. The user also confirmed that a real
Hubstaff stop paused LBX. Automatic resume and failure cases require their own
tests; those observations alone do not establish them.

Observed readiness fields:

```json
{"logged_in":true,"network_online":true,"ready":true}
```

Status fixture using the observed field structure. The ID, name, and duration
below are synthetic mock values, not account data:

```json
{
  "active_project": {
    "id": "mock-project-0001",
    "name": "Mock Hubstaff project",
    "tracked_today": "0:00:00"
  },
  "tracking": true
}
```

The stopped fixture changes `tracking` to false. `tracked_today` is a daily
total, not a current interval duration, and is ignored for local timekeeping.
Optional/missing project fields, login failure, timeouts, malformed output,
oversized output, and stale observations are simulated test cases, not claims
that these exact responses were observed in the installed desktop app.

The mock covers this adapter's interface, not all Hubstaff CLI commands or
Hubstaff's internal accounting. Its poll timing cannot establish the real
desktop app's response latency or behavior across other versions. See
[hubstaff.md](hubstaff.md) for the LBX state-transition contract.
