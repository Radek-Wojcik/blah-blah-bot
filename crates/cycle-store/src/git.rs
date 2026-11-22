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

/// The null device: pointing `core.hooksPath` there means no hook file can be found.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Options placed before every command. A repository is data that other people can change (a
/// synced folder, a shared remote), so what its local configuration or its hooks name must
/// never be started by this application: no hooks, no filesystem monitor, no `ext::` remote.
fn hardening_args() -> Vec<String> {
    [
        format!("core.hooksPath={NULL_DEVICE}"),
        "core.fsmonitor=false".to_string(),
        "protocol.ext.allow=never".to_string(),
        "protocol.file.allow=user".to_string(),
    ]
    .into_iter()
    .flat_map(|setting| ["-c".to_string(), setting])
    .collect()
}

/// Runs `git [-C dir] args...` without ever prompting on a terminal and without running the
/// repository's hooks or helpers (see [`hardening_args`]).
pub(crate) fn run<I, S>(dir: Option<&Path>, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.args(hardening_args());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn every_command_starts_with_the_hardening_options() {
        let args = hardening_args();
        let pairs: Vec<&[String]> = args.chunks(2).collect();
        assert!(pairs.iter().all(|p| p[0] == "-c"));
        let settings: Vec<&str> = pairs.iter().map(|p| p[1].as_str()).collect();
        assert!(settings.contains(&format!("core.hooksPath={NULL_DEVICE}").as_str()));
        assert!(settings.contains(&"core.fsmonitor=false"));
        assert!(settings.contains(&"protocol.ext.allow=never"));
    }

    #[test]
    fn a_hook_and_a_filesystem_monitor_of_the_repository_do_not_run() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let marker = tmp.path().join("ran.txt");
        let marker_arg = marker.to_string_lossy().replace('\\', "/");
        run_ok(Some(&repo), ["init", "--quiet"]).unwrap();
        let hook = repo.join(".git").join("hooks").join("pre-commit");
        fs::write(&hook, format!("#!/bin/sh\necho hook > \"{marker_arg}\"\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(repo.join("a.txt"), "a").unwrap();
        fn add<'a>(args: &[&'a str]) -> Vec<&'a str> {
            let mut full = vec![
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ];
            full.extend_from_slice(args);
            full
        }

        // Control: plain git runs the hook here. When it cannot (no shell for hooks), the
        // behaviour cannot be observed on this machine and the check is skipped.
        let plain = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(add(&[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "control",
            ]))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(plain.status.success());
        if !marker.exists() {
            return;
        }
        fs::remove_file(&marker).unwrap();

        run_ok(
            Some(&repo),
            [
                "config",
                "core.fsmonitor",
                &format!("echo fsm > \"{marker_arg}\""),
            ],
        )
        .unwrap();
        run_ok(
            Some(&repo),
            add(&["commit", "--quiet", "--allow-empty", "-m", "hardened"]),
        )
        .unwrap();
        run_ok(Some(&repo), ["status", "--porcelain"]).unwrap();
        assert!(!marker.exists(), "a repository hook or monitor was started");
    }
}
