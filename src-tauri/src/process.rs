//! Spawning the downloaded tools: argument arrays only (never a shell), no console window
//! on Windows, and killing the whole process tree on cancel (yt-dlp starts ffmpeg).

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Mutex;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use crate::error::{AppError, AppResult};

/// Lines of stderr kept for error classification and the log.
const STDERR_TAIL: usize = 60;

/// Process ids of running tools, so they can be stopped when the app quits.
static RUNNING: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn running() -> std::sync::MutexGuard<'static, Vec<u32>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Stops every running tool (called on app exit; tools run in their own process group
/// and would otherwise outlive the app).
pub fn kill_all() {
    let pids: HashSet<u32> = running().drain(..).collect();
    for pid in pids {
        kill_pid(pid);
    }
}

pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        kill_pid(pid);
    }
    let _ = child.start_kill();
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    // The child leads its own process group (see `command`), so this also stops the
    // ffmpeg it may have started.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .output();
    }
}

/// Resolves once `cancel` is set (or its sender is gone, which also means stop).
pub async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|&c| c).await;
}

/// Output of a finished run: exit status and the last lines of stderr.
pub struct Finished {
    pub status: ExitStatus,
    pub stderr: Vec<String>,
}

/// Runs `cmd`, calling `on_line` for each stdout line, until it exits or `cancel`
/// becomes true (then the process tree is killed and `AppError::Cancelled` returned).
pub async fn run(
    mut cmd: Command,
    cancel: &mut watch::Receiver<bool>,
    mut on_line: impl FnMut(&str),
) -> AppResult<Finished> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let pid = child.id();
    if let Some(pid) = pid {
        running().push(pid);
    }
    let result = wait(&mut child, cancel, &mut on_line).await;
    if let Some(pid) = pid {
        running().retain(|&p| p != pid);
    }
    result
}

async fn wait(
    child: &mut Child,
    cancel: &mut watch::Receiver<bool>,
    on_line: &mut impl FnMut(&str),
) -> AppResult<Finished> {
    // Split on raw bytes: tool output isn't guaranteed to be UTF-8 (Windows code pages).
    let mut out = BufReader::new(child.stdout.take().expect("piped")).split(b'\n');
    let mut err = BufReader::new(child.stderr.take().expect("piped")).split(b'\n');
    let mut tail = VecDeque::with_capacity(STDERR_TAIL);
    let (mut out_open, mut err_open) = (true, true);
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).trim_end().to_string();

    while out_open || err_open {
        tokio::select! {
            () = cancelled(cancel) => {
                kill_tree(child);
                let _ = child.wait().await;
                return Err(AppError::Cancelled);
            }
            line = out.next_segment(), if out_open => match line {
                Ok(Some(l)) => on_line(&text(l)),
                _ => out_open = false,
            },
            line = err.next_segment(), if err_open => match line {
                Ok(Some(l)) => {
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(text(l));
                }
                _ => err_open = false,
            },
        }
    }
    let status = tokio::select! {
        () = cancelled(cancel) => {
            kill_tree(child);
            let _ = child.wait().await;
            return Err(AppError::Cancelled);
        }
        status = child.wait() => status?,
    };
    Ok(Finished {
        status,
        stderr: tail.into(),
    })
}
