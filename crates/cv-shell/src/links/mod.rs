//! Pull request links: whether a connection's token may open one ([`link_access`]) and links
//! copied in another app ([`clipboard_links`]). Reading a URL on a saved connection and what a link
//! in pull request text points at live in `cv_runtime::links`.

#[cfg(test)]
mod access_writes_check;
pub mod clipboard_links;
pub mod link_access;
