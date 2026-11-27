//! The endpoint per system: a named pipe on Windows ([`windows`]), a Unix socket elsewhere
//! ([`unix`]). Each file offers the same surface: [`Endpoint`], [`Listener`], [`Stream`],
//! [`connect`], [`is_absent`] and [`allow_foreground`].

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{Endpoint, Listener, Stream, allow_foreground, connect, is_absent};
#[cfg(windows)]
pub(crate) use windows::{Endpoint, Listener, Stream, allow_foreground, connect, is_absent};
