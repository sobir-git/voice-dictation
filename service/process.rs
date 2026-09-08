use anyhow::{bail, Result};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn exists(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| {
        p.join(program)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}
pub fn nonblocking(fd: &impl AsRawFd) -> std::io::Result<()> {
    // SAFETY: the borrowed descriptor remains open throughout these fcntl calls.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}
struct ChildGuard(Child);
pub fn kill_with_parent(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the pre-exec closure only calls libc syscalls and reads errno.
    // Rechecking the parent closes the race where it exits before prctl runs.
    unsafe {
        let parent = libc::getpid();
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::raise(libc::SIGKILL);
            }
            Ok(())
        });
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
pub fn run(command: &mut Command, input: Option<Vec<u8>>, timeout: Duration) -> Result<Vec<u8>> {
    kill_with_parent(command);
    let mut child = ChildGuard(
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut stdout = child.0.stdout.take().unwrap();
    nonblocking(&stdout)?;
    let mut stdin = child.0.stdin.take();
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    let input = input.unwrap_or_default();
    let mut written = 0;
    let mut output = Vec::new();
    let mut block = [0; 8192];
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(pipe) = &mut stdin {
            if written < input.len() {
                match pipe.write(&input[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if written == input.len() {
                stdin.take();
            }
        }
        loop {
            match stdout.read(&mut block) {
                Ok(0) => break,
                Ok(n) => {
                    output.extend_from_slice(&block[..n]);
                    if output.len() > 64 * 1024 * 1024 {
                        bail!("Command output exceeded 64 MiB")
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(status) = child.0.try_wait()? {
            // Drain bytes already produced; descendants cannot keep us waiting on an inherited pipe.
            loop {
                match stdout.read(&mut block) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        output.extend_from_slice(&block[..n]);
                        if output.len() > 64 * 1024 * 1024 {
                            bail!("Command output exceeded 64 MiB")
                        }
                    }
                }
            }
            if !status.success() {
                bail!("Command exited with {status}")
            }
            if written < input.len() {
                bail!("Command exited before accepting its input")
            }
            return Ok(output);
        }
        if Instant::now() >= deadline {
            bail!("Command timed out")
        }
        thread::sleep(Duration::from_millis(10));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_kills_stalled_child() {
        let before = Instant::now();
        assert!(run(
            Command::new("sleep").arg("5"),
            None,
            Duration::from_millis(50)
        )
        .is_err());
        assert!(before.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn stdin_is_literal_and_output_is_collected() {
        let text = b"hello\nkey ctrl+a\n$(false)".to_vec();
        assert_eq!(
            run(
                &mut Command::new("cat"),
                Some(text.clone()),
                Duration::from_secs(1)
            )
            .unwrap(),
            text
        );
    }
}
