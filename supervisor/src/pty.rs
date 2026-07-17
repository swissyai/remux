// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Narrow PTY process boundary.
//!
//! Contract: allocate one PTY pair per session, attach the child to the slave side,
//! and return its master stream to the supervisor. Process creation happens once per
//! session, never in response to an event.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;

#[link(name = "util")]
unsafe extern "C" {
    fn openpty(
        master: *mut i32,
        slave: *mut i32,
        name: *mut u8,
        termios: *const core::ffi::c_void,
        winsize: *const core::ffi::c_void,
    ) -> i32;
    fn login_tty(file_descriptor: i32) -> i32;
}

unsafe extern "C" {
    fn fcntl(file_descriptor: i32, command: i32, argument: i32) -> i32;
}

pub fn spawn_pty(command: &mut Command) -> io::Result<(Child, File)> {
    let (master, slave) = pty_pair()?;
    let slave_fd = slave.as_raw_fd();
    // SAFETY: the closure calls only async-signal-safe descriptor/session setup before
    // exec. `slave_fd` remains valid in the child because `slave` is held through spawn.
    unsafe {
        command.pre_exec(move || {
            if login_tty(slave_fd) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(slave);
    Ok((child, File::from(master)))
}

fn pty_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // SAFETY: pointers name initialized local integers; null optional outputs request
    // default terminal attributes and window size.
    if unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful `openpty` returned two newly owned descriptors.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    // SAFETY: successful `openpty` returned two newly owned descriptors.
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    set_close_on_exec(&master)?;
    set_close_on_exec(&slave)?;
    Ok((master, slave))
}

fn set_close_on_exec(file: &OwnedFd) -> io::Result<()> {
    // SAFETY: the descriptor is valid and F_SETFD takes an integer bit mask.
    if unsafe { fcntl(file.as_raw_fd(), F_SETFD, FD_CLOEXEC) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
