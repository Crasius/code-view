//! Windows: the named pipe `\\.\pipe\CodeView-<hash of the config folder>`. Each instance of it is
//! created with a DACL that lets only its owner in (`D:P(A;;GA;;;OW)`) and refuses remote clients;
//! the first one also with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so nobody can take the name over while
//! we run. Clients open it at identification level only, so a server can never act as them.
//!
//! The listener always keeps one instance waiting: the next one is created as soon as a client
//! connects, so a start in between finds the pipe busy (and retries) instead of finding no pipe.
#![expect(unsafe_code, reason = "named pipes, their DACL and the foreground right are Win32 calls")]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use review_store::{AppPaths, stable_hash};
use windows_sys::Win32::Foundation::{
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

pub type Stream = File;

/// Full access for the pipe's owner, nothing for anyone else; `P` keeps inherited entries out.
const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";
const BUFFER: u32 = 4096;
/// How long a client waits while every pipe instance is busy (our own budget).
const BUSY_WAIT: Duration = Duration::from_secs(2);
const BUSY_PAUSE: Duration = Duration::from_millis(20);

pub struct Endpoint {
    name: String,
}

impl Endpoint {
    pub fn of(paths: &AppPaths) -> Self {
        let folder = paths.config_dir().to_string_lossy();
        Self { name: format!(r"\\.\pipe\CodeView-{}", stable_hash(&folder)) }
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    let deadline = Instant::now() + BUSY_WAIT;
    loop {
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(&endpoint.name);
        match opened {
            Err(error) if is_busy(&error) && Instant::now() < deadline => pause(),
            other => return other,
        }
    }
}

fn is_busy(error: &io::Error) -> bool {
    error.raw_os_error().and_then(|code| u32::try_from(code).ok()) == Some(ERROR_PIPE_BUSY)
}

#[expect(
    clippy::disallowed_methods,
    reason = "a second start or the native host waits for a busy pipe before any window exists"
)]
fn pause() {
    std::thread::sleep(BUSY_PAUSE);
}

/// No pipe of that name: no instance runs.
pub fn is_absent(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

/// Lets the running instance take the foreground: the user started this process, so it may, and
/// Windows passes the right on only when asked.
pub fn allow_foreground(process_id: u32) {
    // Refused when this process may not take the foreground itself; the window then flashes in
    // the taskbar instead of coming up, which is all Windows allows.
    // SAFETY: takes a plain process id; an id that is no process makes it fail, nothing else.
    let _flashes_instead = unsafe { AllowSetForegroundWindow(process_id) };
}

pub struct Listener {
    name: Vec<u16>,
    waiting: OwnedHandle,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let name: Vec<u16> = endpoint.name.encode_utf16().chain(Some(0)).collect();
        let waiting = create_instance(&name, true)?;
        Ok(Self { name, waiting })
    }

    pub fn accept(&mut self) -> io::Result<Stream> {
        // SAFETY: `waiting` is a pipe handle this listener owns; without an OVERLAPPED the call
        // blocks until a client connects.
        let is_connected =
            unsafe { ConnectNamedPipe(self.waiting.as_raw_handle(), std::ptr::null_mut()) };
        if is_connected == 0 {
            let error = io::Error::last_os_error();
            // A client that connected between creating the instance and this call.
            let is_early = error.raw_os_error().and_then(|code| u32::try_from(code).ok())
                == Some(ERROR_PIPE_CONNECTED);
            if !is_early {
                return Err(error);
            }
        }
        let next = create_instance(&self.name, false)?;
        Ok(File::from(std::mem::replace(&mut self.waiting, next)))
    }
}

/// One instance of the pipe `name` (NUL-terminated), for its owner only.
fn create_instance(name: &[u16], is_first: bool) -> io::Result<OwnedHandle> {
    let descriptor = Descriptor::owner_only()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let open_mode = PIPE_ACCESS_DUPLEX | if is_first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    // SAFETY: `name` is NUL-terminated and `attributes` points to a descriptor that lives until
    // after the call; both outlive it.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            pipe_mode,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            &raw const attributes,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a valid handle that CreateNamedPipeW just returned and nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// A security descriptor from SDDL, freed with `LocalFree` as its API asks.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn owner_only() -> io::Result<Self> {
        let sddl: Vec<u16> = OWNER_ONLY.encode_utf16().chain(Some(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; the call writes one pointer to `descriptor` and accepts
        // a null size pointer.
        let is_converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if is_converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // Non-null only when freeing failed; a leaked descriptor of a few bytes harms nothing.
        // SAFETY: the descriptor came from ConvertStringSecurityDescriptorToSecurityDescriptorW,
        // which allocates with LocalAlloc, and is freed only here.
        let _leaked = unsafe { LocalFree(self.0) };
    }
}
