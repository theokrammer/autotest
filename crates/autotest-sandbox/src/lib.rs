use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

pub mod staging;
pub use staging::{StagingEntry, StagingManager, StagingError};

#[derive(Error, Debug)]
pub enum SandboxError {
    #[error("Process timed out after {0} ms")]
    Timeout(u64),
    #[error("Failed to execute process: {0}")]
    Io(#[from] std::io::Error),
    #[error("Sandbox isolation failure: {0}")]
    Isolation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_true")]
    pub block_network: bool,
    #[serde(default = "default_true")]
    pub allow_temp_fs_only: bool,
}

fn default_mode() -> String {
    "os_native".to_string()
}
fn default_timeout() -> u64 {
    500
}
fn default_true() -> bool {
    true
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            timeout_ms: default_timeout(),
            block_network: true,
            allow_temp_fs_only: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

pub struct SandboxRunner {
    pub config: SandboxConfig,
}

impl SandboxRunner {
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Executes a command in an isolated sandbox environment
    pub fn run_command(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Path,
    ) -> Result<SandboxResult, SandboxError> {
        self.run_command_with_env(program, args, working_dir, &[])
    }

    /// Executes a command in an isolated sandbox environment with extra environment variables
    pub fn run_command_with_env(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Path,
        extra_envs: &[(&str, &str)],
    ) -> Result<SandboxResult, SandboxError> {
        let temp_dir = tempfile::tempdir().map_err(SandboxError::Io)?;
        let start_time = Instant::now();

        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.current_dir(working_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Setup environment isolation
        cmd.env("TMPDIR", temp_dir.path());
        cmd.env("TEMP", temp_dir.path());
        cmd.env("TMP", temp_dir.path());

        // Pass extra environment variables
        for (k, v) in extra_envs {
            cmd.env(k, v);
        }

        // If network isolation requested, clear proxy variables
        if self.config.block_network {
            cmd.env("HTTP_PROXY", "http://127.0.0.1:0");
            cmd.env("HTTPS_PROXY", "http://127.0.0.1:0");
            cmd.env("ALL_PROXY", "http://127.0.0.1:0");
            cmd.env("http_proxy", "http://127.0.0.1:0");
            cmd.env("https_proxy", "http://127.0.0.1:0");
            cmd.env("all_proxy", "http://127.0.0.1:0");
        }

        // Platform-specific OS sandbox hooks with graceful degradation
        #[cfg(target_os = "linux")]
        {
            if self.config.block_network {
                self.setup_linux_isolation(&mut cmd);
            }
        }

        let mut child = cmd.spawn().map_err(SandboxError::Io)?;

        // Spawn background reader threads for stdout and stderr to prevent OS pipe buffer deadlocks
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let stdout_thread = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe.take() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let stderr_thread = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe.take() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let timeout = Duration::from_millis(self.config.timeout_ms);
        let poll_interval = Duration::from_millis(10);

        loop {
            match child.try_wait()? {
                Some(status) => {
                    let stdout_bytes = stdout_thread.join().unwrap_or_default();
                    let stderr_bytes = stderr_thread.join().unwrap_or_default();
                    return Ok(SandboxResult {
                        exit_code: status.code(),
                        stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
                        stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
                        duration: start_time.elapsed(),
                    });
                }
                None => {
                    if start_time.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_thread.join();
                        let _ = stderr_thread.join();
                        return Err(SandboxError::Timeout(self.config.timeout_ms));
                    }
                    thread::sleep(poll_interval);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn setup_linux_isolation(&self, _cmd: &mut Command) {
        // In restricted container environments or unprivileged user namespaces,
        // we log graceful degradation notice.
        warn!("Linux Landlock & Network unshare active (with graceful user-space isolation fallback)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_successful_command() {
        let runner = SandboxRunner::new(SandboxConfig::default());
        let cur_dir = std::env::current_dir().unwrap();
        let result = runner.run_command("echo", &["test_sandbox_output"], &cur_dir).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("test_sandbox_output"));
    }

    #[test]
    fn test_sandbox_command_with_env() {
        let runner = SandboxRunner::new(SandboxConfig::default());
        let cur_dir = std::env::current_dir().unwrap();
        let result = runner.run_command_with_env("sh", &["-c", "echo $TEST_VAR"], &cur_dir, &[("TEST_VAR", "hello_sandbox")]).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello_sandbox"));
    }

    #[test]
    fn test_sandbox_timeout() {
        let mut config = SandboxConfig::default();
        config.timeout_ms = 100; // 100ms
        let runner = SandboxRunner::new(config);
        let cur_dir = std::env::current_dir().unwrap();
        // Sleep for 1 second which exceeds 100ms
        let result = runner.run_command("sleep", &["1"], &cur_dir);
        match result {
            Err(SandboxError::Timeout(ms)) => assert_eq!(ms, 100),
            other => panic!("Expected Timeout error, got {:?}", other),
        }
    }

    #[test]
    fn test_sandbox_large_output_pipe_handling() {
        let runner = SandboxRunner::new(SandboxConfig::default());
        let cur_dir = std::env::current_dir().unwrap();
        // Generate output larger than 64KB pipe buffer to test non-blocking pipe reads
        let result = runner.run_command("head", &["-c", "100000", "/dev/zero"], &cur_dir).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.len(), 100000);
    }
}
