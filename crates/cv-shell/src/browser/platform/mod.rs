//! Where the browsers look for native messaging hosts per system. Only Windows needs code of its
//! own (the registry, [`windows`]); macOS and Linux read plain files, which `registration` writes.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::UserKeys;
