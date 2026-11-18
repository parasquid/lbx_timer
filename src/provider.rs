//! Read-only observation of Hubstaff's local CLI.
//!
//! The timer never sends Hubstaff control commands. The adapter runs only
//! `send wait-login` and `send status`, and uses Hubstaff's local ready/login
//! state without requiring cloud connectivity.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

pub const HUBSTAFF_POLL_INTERVAL: Duration = Duration::from_secs(2);
// Allow a routine interval, the complete query timeout, and scheduling margin.
pub const HUBSTAFF_FRESHNESS: Duration = Duration::from_secs(6);
const HUBSTAFF_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const CLI_NAME: &str = "HubstaffCLI.bin.x86_64";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Standalone,
    Hubstaff,
}

pub trait TimeProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StandaloneProvider;

impl TimeProvider for StandaloneProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Standalone
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserverConfig {
    Standalone,
    Hubstaff(PathBuf),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryEnv {
    pub standalone: bool,
    pub explicit_cli: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub path: Option<OsString>,
}

impl DiscoveryEnv {
    pub fn from_process() -> Self {
        let standalone = std::env::args().skip(1).any(|arg| arg == "--standalone");
        Self {
            standalone,
            explicit_cli: std::env::var_os("LBX_HUBSTAFF_CLI").map(PathBuf::from),
            home: std::env::var_os("HOME").map(PathBuf::from),
            path: std::env::var_os("PATH"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubstaffSnapshot {
    pub tracking: bool,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubstaffStatus {
    Checking,
    Tracking { project: Option<String> },
    Stopped { project: Option<String> },
    Unavailable { reason: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{0}")]
pub struct ProviderError(pub String);

pub fn discover(env: DiscoveryEnv) -> ObserverConfig {
    if env.standalone {
        return ObserverConfig::Standalone;
    }
    if let Some(explicit) = env.explicit_cli {
        return if is_executable(&explicit) {
            ObserverConfig::Hubstaff(explicit)
        } else {
            ObserverConfig::Unavailable(
                "The configured Hubstaff CLI is missing or not executable.".to_owned(),
            )
        };
    }

    if let Some(home) = env.home {
        let candidate = home.join("Hubstaff").join(CLI_NAME);
        if is_executable(&candidate) {
            return ObserverConfig::Hubstaff(candidate);
        }
    }
    if let Some(search_path) = env.path {
        for directory in std::env::split_paths(&search_path) {
            let candidate = directory.join(CLI_NAME);
            if is_executable(&candidate) {
                return ObserverConfig::Hubstaff(candidate);
            }
        }
    }
    ObserverConfig::Standalone
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn parse_wait_login(output: &str) -> Result<(), ProviderError> {
    let response: serde_json::Value = serde_json::from_str(output)
        .map_err(|_| ProviderError("Hubstaff returned an invalid readiness response.".into()))?;
    let logged_in = response
        .get("logged_in")
        .and_then(serde_json::Value::as_bool);
    let ready = response.get("ready").and_then(serde_json::Value::as_bool);
    if logged_in == Some(true) && ready == Some(true) {
        Ok(())
    } else {
        Err(ProviderError(
            "Hubstaff is not logged in and ready for local tracking.".into(),
        ))
    }
}

pub fn parse_status(output: &str) -> Result<HubstaffSnapshot, ProviderError> {
    let response: serde_json::Value = serde_json::from_str(output)
        .map_err(|_| ProviderError("Hubstaff returned an invalid status response.".into()))?;
    let tracking = response
        .get("tracking")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| ProviderError("Hubstaff status omitted its tracking state.".into()))?;
    let project = response
        .get("active_project")
        .and_then(|value| value.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(HubstaffSnapshot { tracking, project })
}

pub async fn poll(path: &Path) -> Result<HubstaffSnapshot, ProviderError> {
    run_poll(path, HUBSTAFF_QUERY_TIMEOUT).await
}

/// Once readiness is established, a healthy poll needs only desktop status.
/// Any failure invalidates the caller's readiness cache before retrying.
pub async fn poll_status(path: &Path) -> Result<HubstaffSnapshot, ProviderError> {
    run_poll_with_readiness(path, HUBSTAFF_QUERY_TIMEOUT, false).await
}

/// Delay after consecutive failures, measured from completion of the failed
/// attempt: 2, 4, 8, 16, then 30 seconds. Explicit starts bypass this delay.
pub fn retry_delay(failures: u32) -> Duration {
    HUBSTAFF_POLL_INTERVAL
        .saturating_mul(1 << failures.saturating_sub(1).min(4))
        .min(Duration::from_secs(30))
}

async fn run_poll(path: &Path, timeout: Duration) -> Result<HubstaffSnapshot, ProviderError> {
    run_poll_with_readiness(path, timeout, true).await
}

async fn run_poll_with_readiness(
    path: &Path,
    timeout: Duration,
    check_readiness: bool,
) -> Result<HubstaffSnapshot, ProviderError> {
    tokio::time::timeout(timeout, async {
        if check_readiness {
            let readiness = run_command(path, &["send", "wait-login"]).await?;
            parse_wait_login(&readiness)?;
        }
        let status = run_command(path, &["send", "status"]).await?;
        parse_status(&status)
    })
    .await
    .map_err(|_| ProviderError("Hubstaff status query timed out.".into()))?
}

async fn run_command(path: &Path, args: &[&str]) -> Result<String, ProviderError> {
    let mut child = Command::new(path)
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderError(format!(
                "Could not run the Hubstaff CLI ({:?}: {error}).",
                error.kind()
            ))
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError("Could not read Hubstaff CLI output.".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderError("Could not read Hubstaff CLI output.".into()))?;

    let mut readers = tokio::task::JoinSet::new();
    readers.spawn(async move { (true, read_capped(stdout).await) });
    readers.spawn(async move { (false, read_capped(stderr).await) });
    let status = child.wait().await;
    let mut stdout = None;
    let mut overflowed = false;
    while let Some(result) = readers.join_next().await {
        let (is_stdout, output) =
            result.map_err(|_| ProviderError("Could not read Hubstaff CLI output.".into()))?;
        let output = output?;
        overflowed |= output.overflowed;
        if is_stdout {
            stdout = Some(output.bytes);
        }
    }
    let status = status.map_err(|_| ProviderError("Hubstaff CLI exited unexpectedly.".into()))?;
    if !status.success() {
        return Err(ProviderError("Hubstaff CLI command failed.".into()));
    }
    if overflowed {
        return Err(ProviderError(
            "Hubstaff CLI output exceeded the supported size limit.".into(),
        ));
    }
    String::from_utf8(stdout.unwrap_or_default())
        .map_err(|_| ProviderError("Hubstaff CLI returned non-text output.".into()))
}

struct CappedOutput {
    bytes: Vec<u8>,
    overflowed: bool,
}

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R) -> Result<CappedOutput, ProviderError> {
    let mut output = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0; 4096];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|_| ProviderError("Could not read Hubstaff CLI output.".into()))?;
        if read == 0 {
            break;
        }
        let keep = read.min(MAX_OUTPUT_BYTES.saturating_sub(output.len()));
        overflowed |= keep < read;
        output.extend_from_slice(&buffer[..keep]);
    }
    Ok(CappedOutput {
        bytes: output,
        overflowed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::ReadBuf;
    use uuid::Uuid;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("lbx-provider-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn executable(path: &PathBuf, body: &str) {
        fs::write(path, body).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[test]
    fn readiness_requires_local_login_and_ready_but_not_cloud_connectivity() {
        assert!(
            parse_wait_login(r#"{"logged_in":true,"network_online":false,"ready":true}"#).is_ok()
        );
        assert!(parse_wait_login(r#"{"logged_in":false,"ready":true}"#).is_err());
        assert!(parse_wait_login(r#"{"logged_in":true,"ready":false}"#).is_err());
        assert!(parse_wait_login("not json").is_err());
    }

    #[test]
    fn status_requires_tracking_boolean_and_tolerates_missing_project() {
        let tracking = parse_status(r#"{"tracking":true}"#).unwrap();
        assert!(tracking.tracking);
        assert_eq!(tracking.project, None);

        let stopped = parse_status(
            r#"{"tracking":false,"active_project":{"id":4210266,"name":"Code Behaviors","tracked_today":"0:43:53"}}"#,
        )
        .unwrap();
        assert!(!stopped.tracking);
        assert_eq!(stopped.project.as_deref(), Some("Code Behaviors"));
        assert!(parse_status(r#"{"active_project":null}"#).is_err());
        assert!(parse_status("{} garbage").is_err());
    }

    #[test]
    fn spawn_errors_preserve_the_operating_system_error_kind() {
        let temp = TempDir::new();
        let error = run_poll_blocking(&temp.0.join("missing-cli"), Duration::from_secs(1))
            .expect_err("missing executable must be unavailable");
        assert!(error.0.contains("NotFound"), "{error}");
    }

    #[test]
    fn discovery_honors_standalone_override_and_never_falls_back_from_bad_explicit_cli() {
        let temp = TempDir::new();
        let hubstaff_dir = temp.0.join("Hubstaff");
        fs::create_dir(&hubstaff_dir).unwrap();
        let cli = hubstaff_dir.join("HubstaffCLI.bin.x86_64");
        executable(&cli, "#!/bin/sh\nexit 0\n");

        assert_eq!(
            discover(DiscoveryEnv {
                standalone: true,
                explicit_cli: Some(temp.0.join("missing")),
                home: Some(temp.0.clone()),
                path: None,
            }),
            ObserverConfig::Standalone
        );
        assert!(matches!(
            discover(DiscoveryEnv {
                standalone: false,
                explicit_cli: Some(temp.0.join("missing")),
                home: Some(temp.0.clone()),
                path: None,
            }),
            ObserverConfig::Unavailable(_)
        ));
        assert_eq!(
            discover(DiscoveryEnv {
                standalone: false,
                explicit_cli: None,
                home: Some(temp.0.clone()),
                path: None,
            }),
            ObserverConfig::Hubstaff(cli.clone())
        );
        assert_eq!(
            discover(DiscoveryEnv {
                standalone: false,
                explicit_cli: None,
                home: None,
                path: Some(OsString::from(hubstaff_dir.as_os_str())),
            }),
            ObserverConfig::Hubstaff(cli)
        );
        assert_eq!(
            discover(DiscoveryEnv {
                standalone: false,
                explicit_cli: None,
                home: None,
                path: None,
            }),
            ObserverConfig::Standalone
        );
    }

    #[cfg(unix)]
    #[test]
    fn polling_uses_only_fixed_read_commands_and_parses_both_responses() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        let argv = temp.0.join("argv");
        executable(
            &cli,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$2\" = wait-login ]; then echo '{{\"logged_in\":true,\"ready\":true,\"network_online\":false}}'; else echo '{{\"tracking\":true,\"active_project\":{{\"name\":\"Review\"}}}}'; fi\n",
                argv.display()
            ),
        );

        let result = run_poll_blocking(&cli, Duration::from_secs(1));
        let result = result.unwrap();
        assert!(result.tracking);
        assert_eq!(result.project.as_deref(), Some("Review"));
        assert_eq!(
            fs::read_to_string(argv).unwrap(),
            "send wait-login\nsend status\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn healthy_status_polls_skip_readiness_until_it_is_requested_again() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        let argv = temp.0.join("argv");
        executable(&cli, &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$2\" = wait-login ]; then echo '{{\"logged_in\":true,\"ready\":true}}'; else echo '{{\"tracking\":true}}'; fi\n",
            argv.display()
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(poll(&cli).await.unwrap().tracking);
            assert!(poll_status(&cli).await.unwrap().tracking);
            assert!(poll_status(&cli).await.unwrap().tracking);
            assert!(poll(&cli).await.unwrap().tracking);
        });
        assert_eq!(fs::read_to_string(argv).unwrap(), "send wait-login\nsend status\nsend status\nsend status\nsend wait-login\nsend status\n");
    }

    #[cfg(unix)]
    #[test]
    fn status_only_polls_still_reject_invalid_responses_and_timeout() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for body in [
            "#!/bin/sh\necho '{bad json'\n",
            "#!/bin/sh\nexit 3\n",
            "#!/bin/sh\necho '{}'\n",
            "#!/bin/sh\nexec sleep 2\n",
        ] {
            executable(&cli, body);
            assert!(runtime
                .block_on(run_poll_with_readiness(
                    &cli,
                    Duration::from_millis(50),
                    false
                ))
                .is_err());
        }
    }

    #[test]
    fn failed_polls_back_off_to_a_bounded_retry_interval() {
        let delays: Vec<_> = [1, 2, 3, 4, 5, u32::MAX]
            .into_iter()
            .map(|failures| retry_delay(failures).as_secs())
            .collect();
        assert_eq!(delays, [2, 4, 8, 16, 30, 30]);
    }

    #[cfg(unix)]
    #[test]
    fn polling_returns_unavailable_on_cli_errors_malformed_json_and_timeout() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        executable(&cli, "#!/bin/sh\nexit 3\n");
        assert!(run_poll_blocking(&cli, Duration::from_secs(1)).is_err());

        executable(
            &cli,
            "#!/bin/sh\nif [ \"$2\" = wait-login ]; then echo '{\"logged_in\":true,\"ready\":true}'; else echo '{bad json'; fi\n",
        );
        assert!(run_poll_blocking(&cli, Duration::from_secs(1)).is_err());

        executable(&cli, "#!/bin/sh\nexec sleep 2\n");
        let started = std::time::Instant::now();
        assert!(run_poll_blocking(&cli, Duration::from_millis(50)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn oversized_stdout_and_stderr_are_rejected_and_do_not_block_the_reader() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        let valid_prefix = r#"{"logged_in":true,"ready":true}"#;
        let capped_prefix = format!(
            "{valid_prefix}{}",
            " ".repeat(MAX_OUTPUT_BYTES - valid_prefix.len())
        );
        assert!(serde_json::from_str::<serde_json::Value>(&capped_prefix).is_ok());
        executable(
            &cli,
            r#"#!/bin/sh
if [ "$2" = wait-login ]; then
    printf '%s' '{"logged_in":true,"ready":true}'
    head -c 65536 /dev/zero | tr '\000' ' '
    printf 'garbage'
else
    echo '{"tracking":true}'
fi
"#,
        );
        let started = std::time::Instant::now();
        assert!(run_poll_blocking(&cli, Duration::from_secs(1)).is_err());
        assert!(started.elapsed() < Duration::from_millis(900));

        executable(
            &cli,
            r#"#!/bin/sh
if [ "$2" = wait-login ]; then
    echo '{"logged_in":true,"ready":true}'
    head -c 65537 /dev/zero | tr '\000' ' ' >&2
else
    echo '{"tracking":true}'
fi
"#,
        );
        let started = std::time::Instant::now();
        assert!(run_poll_blocking(&cli, Duration::from_secs(1)).is_err());
        assert!(started.elapsed() < Duration::from_millis(900));
    }

    #[test]
    fn capped_reader_surfaces_stream_errors() {
        struct ReadFailure;

        impl AsyncRead for ReadFailure {
            fn poll_read(
                self: Pin<&mut Self>,
                _context: &mut Context<'_>,
                _buffer: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Err(std::io::Error::other("simulated pipe error")))
            }
        }

        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(read_capped(ReadFailure));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_not_ready_cli_is_unavailable_without_running_the_status_command() {
        let temp = TempDir::new();
        let cli = temp.0.join("cli");
        let argv = temp.0.join("argv");
        executable(
            &cli,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\necho '{{\"logged_in\":false,\"ready\":true}}'\n",
                argv.display()
            ),
        );
        assert!(run_poll_blocking(&cli, Duration::from_secs(1)).is_err());
        assert_eq!(fs::read_to_string(argv).unwrap(), "send wait-login\n");
    }

    #[cfg(unix)]
    #[test]
    fn mock_cli_states_are_consumed_by_public_poll_without_mutating_its_state_file() {
        let temp = TempDir::new();
        let mock = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/mock-hubstaff.py");
        let state = temp.0.join("mock-state.json");
        let wrapper = temp.0.join("mock-cli");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        executable(
            &wrapper,
            &format!(
                "#!/bin/sh\nexport LBX_MOCK_HUBSTAFF_STATE={}\nexec {} \"$@\"\n",
                quote(&state.display().to_string()),
                quote(&mock.display().to_string())
            ),
        );

        let set_mode = |mode: &str| {
            std::process::Command::new("python3")
                .arg(&mock)
                .args(["set", mode])
                .env("LBX_MOCK_HUBSTAFF_STATE", &state)
                .output()
                .expect("run the isolated mock controller")
        };

        assert!(set_mode("running").status.success());
        let before_read = fs::read(&state).unwrap();
        let running = run_poll_blocking(&wrapper, Duration::from_secs(1)).unwrap();
        assert!(running.tracking);
        assert_eq!(running.project.as_deref(), Some("Mock Hubstaff project"));
        assert_eq!(fs::read(&state).unwrap(), before_read);

        assert!(set_mode("stopped").status.success());
        let stopped = run_poll_blocking(&wrapper, Duration::from_secs(1)).unwrap();
        assert!(!stopped.tracking);

        assert!(set_mode("unavailable").status.success());
        assert!(run_poll_blocking(&wrapper, Duration::from_secs(1)).is_err());
    }

    fn run_poll_blocking(
        path: &Path,
        timeout: Duration,
    ) -> Result<HubstaffSnapshot, ProviderError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_poll(path, timeout))
    }
}
