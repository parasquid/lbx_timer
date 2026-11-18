# Development

The normal user path is `cargo run` from the project root. This page collects
optional toolchain, build, test, and isolated-data instructions.

## Requirements

- Rust 1.89 or newer, with Cargo, rustfmt, and Clippy.
- A C compiler/linker for bundled SQLite and native dependencies.
- A graphical Linux session using Wayland or X11.

SQLite is bundled. Depending on the Linux distribution, native runtime/build
libraries may also be needed. Common Ubuntu/Debian prerequisites include
`build-essential`, `pkg-config`, `libxkbcommon-dev`, `libfontconfig1-dev`,
`libwayland-dev`, `libx11-dev`, `libxcursor-dev`, `libxi-dev`, `libxrandr-dev`,
and `libgl1-mesa-dev`. Windows/WSL2 and macOS have not been validated.

## Toolchain setup

`rust-toolchain.toml` selects the installed stable toolchain for this project,
so a rustup global default is not required. The repository setup helper is
optional and discovers or prepares a suitable toolchain when Cargo is not
already available:

```bash
source scripts/setup.sh --no-profile
```

The helper may install distribution packages when explicitly requested and can
inspect the environment without changing profiles or installing packages:

```bash
scripts/setup.sh --help
scripts/setup.sh --find
scripts/setup.sh --check
eval "$(scripts/setup.sh --path)"
```

Use the discovered toolchain only for the current shell; do not change shell
profiles or trust configuration just to run the app.

## Build and run

```bash
cargo run
cargo build --release
./target/release/lbx-timer
```

Use a disposable database while experimenting:

```bash
cargo run -- --db /tmp/lbx-timer-demo.sqlite3
# Or set the database path for the process:
LBX_TIMER_DB=/path/to/timer.sqlite3 cargo run
```

## Checks

```bash
cargo test --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
bash -n scripts/setup.sh
```

The UI simulator uses a headless renderer. Set `ICED_TEST_BACKEND=tiny-skia`
when forcing that backend in a constrained environment:

```bash
ICED_TEST_BACKEND=tiny-skia cargo test --locked --all-targets
```

## Troubleshooting: "failed to select a version ... (locked to x.y.z)"

```
error: failed to select a version for the requirement `uuid = "^1"` (locked to 1.29.1)
```

`Cargo.lock` pins an exact version, and the registry Cargo is reading does not
offer it. Usually the registry view is behind: sandboxed or proxied
environments often serve a crates.io mirror that lags the real index, so a
lockfile generated on an unrestricted machine can pin releases the mirror does
not have yet. Less often the pinned version was never published (a hand-edited
lockfile). `Cargo.toml` only asks for `uuid = "1"`, so the pin is the sole
constraint. Read the rest of the error first:

- `candidate versions found which didn't match:` shows what the registry does
  have. If the newest candidate is older than the locked version, either the
  version does not exist or this registry view is behind.
- `location searched:` names the source. A vendor directory or a mirror there
  means the lockfile was produced against a different registry.
- A reminder about offline mode means Cargo only considered cached crates.

To tell the two apart, ask crates.io directly from a machine with unrestricted
network access, bypassing Cargo's cache and any mirror. The version exists only
if the second command prints a line:

```bash
curl -s https://crates.io/api/v1/crates/uuid | grep -o '"max_version":"[^"]*"'
curl -s https://index.crates.io/uu/id/uuid | grep '"vers":"1.29.1"'
```

Check for offline or source-replacement settings:

```bash
env | grep -i cargo          # CARGO_NET_OFFLINE, CARGO_HOME, CARGO_UNSTABLE_*
cat .cargo/config.toml ~/.cargo/config.toml 2>/dev/null
```

With network access, refresh just that crate (any `1.x` works; `uuid` is used
only for v4 IDs):

```bash
cargo update -p uuid
cargo build            # no --locked / --offline / --frozen while fixing this
```

If Cargo still reports older candidates, the local index cache may be stale:

```bash
rm -rf "${CARGO_HOME:-$HOME/.cargo}/registry/index"/*/.cache
cargo update -p uuid
```

Clearing the cache cannot help when the mirror itself is behind. Then, or
without network access, either vendor the dependencies on an unrestricted
machine (`cargo vendor`, then commit the generated `.cargo/config.toml`), or
pin the crate to a version the available registry has. An older patch release
of the same minor version is a safe substitute:

```bash
cargo update -p uuid --precise <version-the-registry-has>
```

Regenerating the whole lockfile (`rm Cargo.lock && cargo generate-lockfile`)
is the last resort: it moves every dependency and needs a full re-verification.

Tests use disposable databases, never your own. What they cover, the review
snapshots, and the manual smoke test are described in [testing.md](testing.md).
Add `--offline` only when the required packages and index entries are already
cached.

## Code layout

The library crate holds everything that is not UI and is tested without a
window:

- `storage.rs`: the SQLite schema and all queries. Every change that belongs
  together runs in one transaction.
- `timer.rs`: `TimerEngine`, the current session's linked timers, time
  accounting (monotonic clock, checkpoints, sleep detection), and resuming
  sessions.
- `breakdown.rs`: segments and timelines derived from events.
- `event_log.rs`: the readable sentence for each event.
- `provider.rs`: read-only Hubstaff CLI observation, validation, discovery, and
  freshness. See [hubstaff.md](hubstaff.md) for setup and failure behavior.
- `verification.rs`: local audit evidence, pending/confirmed/adjusted intervals,
  and exclusions shared by time accounting, Details, and the session timeline.

Database compatibility is required for every release from schema 5 onward.
New builds automatically migrate supported databases in a transaction. Keep
earlier migration paths and test their data retention and reopening. Schema 6
adds verification metadata without rewriting timer or event rows. Original
local time is retained; query results and live totals apply saved exclusions.
Unknown future schemas must be refused before any writable connection opens.

The binary is the iced app:

- `app/mod.rs`: `App` state, messages, and update logic.
- `app/view/`: one module per area (clock card, timer list, Details, History,
  dialogs), plus small shared view pieces.
- `app/format.rs`: pure text formatting. `app/metrics.rs`: sizes, the type
  scale, and spacing; views use these names instead of bare numbers.
  `app/style.rs`: colors and style functions.
- `widgets/`: reusable widgets that know nothing about the app (vector icons,
  select-on-focus text fields).
