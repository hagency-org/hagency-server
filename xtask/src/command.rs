use anyhow::{Result, bail};
use std::{
    os::unix::process::CommandExt,
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

// Give child commands their own process group. The watcher owns shutdown, including
// compiler/frontend grandchildren, and keeps the old server alive on failed builds.
pub fn spawn(command: &mut Command) -> Result<Child> {
    Ok(command.process_group(0).spawn()?)
}

pub fn stop(child: &mut Child) -> Result<()> {
    stop_with_grace(child, Duration::from_secs(20))
}

pub fn stop_with_grace(child: &mut Child, grace: Duration) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    signal(child.id(), libc::SIGTERM);
    let deadline = Instant::now() + grace;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            signal(child.id(), libc::SIGKILL);
            child.wait()?;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn signal(pid: u32, signal: i32) {
    // SAFETY: a negative child PID denotes its process group, established by spawn.
    unsafe { libc::kill(-(pid as i32), signal) };
}

pub fn run(command: &mut Command, stopping: &AtomicBool) -> Result<bool> {
    let mut child = spawn(command)?;
    loop {
        if stopping.load(Ordering::Relaxed) {
            stop(&mut child)?;
            return Ok(false);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn checked(command: &mut Command, stopping: &AtomicBool) -> Result<()> {
    if !run(command, stopping)? {
        bail!("Command failed or was interrupted: {command:?}");
    }
    Ok(())
}

pub fn output(command: &mut Command) -> Result<Output> {
    // Output commands are short-lived metadata/config queries, not server processes.
    let output = command.stdout(Stdio::piped()).output()?;
    if !output.status.success() {
        bail!("Command failed: {command:?}");
    }
    Ok(output)
}
