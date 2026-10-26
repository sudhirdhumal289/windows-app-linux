//! User commands run before a session connects and after it ends (e.g. starting a VPN
//! or mounting a share). Commands run through the system shell with details of the
//! connection in `WINDOWS_APP_*` environment variables.

use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a pre-connect command may run before the connection is abandoned.
pub const PRE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Details exposed to commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookContext {
    /// Connection name shown in the app.
    pub name: String,
    /// `host:port` for Remote PCs; empty for Cloud PCs and Azure Virtual Desktop.
    pub address: String,
    /// Launch target (`remote:<id>`, `cloudpc:<id>`, `avd:<id>`).
    pub target: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("Could not run the command: {0}")]
    Spawn(io::Error),
    #[error("The command did not finish within {} seconds", .0.as_secs())]
    Timeout(Duration),
    #[error("The command failed (exit status {0})")]
    Failed(String),
}

fn shell(command: &str, context: &HookContext, phase: &str) -> Command {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW prevents command prompt flash
        }
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.env("WINDOWS_APP_PHASE", phase)
        .env("WINDOWS_APP_NAME", &context.name)
        .env("WINDOWS_APP_ADDRESS", &context.address)
        .env("WINDOWS_APP_TARGET", &context.target)
        .stdin(Stdio::null());
    cmd
}

/// Runs `command` and waits for it to succeed, killing it after `timeout`. Blocking.
pub fn run_pre_connect(
    command: &str,
    context: &HookContext,
    timeout: Duration,
) -> Result<(), HookError> {
    let mut child = shell(command, context, "pre-connect")
        .spawn()
        .map_err(HookError::Spawn)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(HookError::Spawn)? {
            return if status.success() {
                Ok(())
            } else {
                Err(HookError::Failed(
                    status
                        .code()
                        .map_or_else(|| "signal".to_string(), |c| c.to_string()),
                ))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HookError::Timeout(timeout));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts `command` without waiting for it (its exit status is logged).
pub fn spawn_post_disconnect(command: &str, context: &HookContext) -> Result<(), HookError> {
    let mut child = shell(command, context, "post-disconnect")
        .spawn()
        .map_err(HookError::Spawn)?;
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => log::warn!("Post-disconnect command: {status}"),
        Err(e) => log::warn!("Post-disconnect command: {e}"),
        Ok(_) => {}
    });
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn ctx() -> HookContext {
        HookContext {
            name: "Office PC".into(),
            address: "pc:3389".into(),
            target: "remote:1".into(),
        }
    }

    #[test]
    fn pre_command_succeeds_and_sees_context() {
        let cmd =
            r#"test "$WINDOWS_APP_NAME" = "Office PC" && test "$WINDOWS_APP_PHASE" = pre-connect"#;
        run_pre_connect(cmd, &ctx(), Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn pre_command_failure_and_timeout() {
        assert!(matches!(
            run_pre_connect("exit 3", &ctx(), Duration::from_secs(5)),
            Err(HookError::Failed(code)) if code == "3"
        ));
        assert!(matches!(
            run_pre_connect("sleep 5", &ctx(), Duration::from_millis(200)),
            Err(HookError::Timeout(_))
        ));
    }

    #[test]
    fn post_command_is_detached() {
        spawn_post_disconnect("true", &ctx()).unwrap();
    }
}
