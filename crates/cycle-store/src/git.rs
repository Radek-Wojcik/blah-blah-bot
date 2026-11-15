//! Thin wrapper over the `git` command line.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{CycleError, Result};

pub(crate) struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The error text of a failed command: stderr, or stdout when stderr is empty.
    pub fn message(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let text = text.trim();
        // Keep the recorded message short; the first lines carry the cause.
        text.lines().take(6).collect::<Vec<_>>().join("\n")
    }
}

/// Runs `git [-C dir] args...` without ever prompting on a terminal.
pub(crate) fn run<I, S>(dir: Option<&Path>, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up when the desktop app runs git.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(CycleError::GitUnavailable)?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Runs git and turns a non-zero exit into [`CycleError::Git`]; returns trimmed stdout.
pub(crate) fn run_ok<I, S>(dir: Option<&Path>, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().collect();
    let command = args
        .iter()
        .take(2)
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let out = run(dir, &args)?;
    if out.ok {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(CycleError::Git {
            command,
            message: out.message(),
        })
    }
}

/// Whether a `git` executable can be started.
pub fn git_available() -> bool {
    run(None, ["--version"]).map(|o| o.ok).unwrap_or(false)
}
