//! macOS, Linux and the BSDs: a Unix socket in `ipc/` of the cache folder. The folder is the
//! owner's alone (mode 0700), so nobody else can reach the socket in it.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use review_store::AppPaths;

pub type Stream = UnixStream;

pub struct Endpoint {
    socket: PathBuf,
}

impl Endpoint {
    pub fn of(paths: &AppPaths) -> Self {
        Self { socket: paths.ipc_socket() }
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    UnixStream::connect(&endpoint.socket)
}

/// No socket, or one a crashed instance left that nobody answers on.
pub fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

/// The window manager decides on its own; there is no right to hand over.
pub fn allow_foreground(_process_id: u32) {}

pub struct Listener(UnixListener);

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let folder = endpoint
            .socket
            .parent()
            .ok_or_else(|| io::Error::other("socket path has no folder"))?;
        fs::create_dir_all(folder)?;
        fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
        if endpoint.socket.exists() {
            if UnixStream::connect(&endpoint.socket).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, "another instance answers"));
            }
            // Left by an instance that crashed: nobody answers on it.
            fs::remove_file(&endpoint.socket)?;
        }
        UnixListener::bind(&endpoint.socket).map(Self)
    }

    pub fn accept(&mut self) -> io::Result<Stream> {
        self.0.accept().map(|(stream, _)| stream)
    }
}
