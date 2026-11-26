//! What a link or a reference in text points at: a pull request URL of a connection
//! ([`pull_request_link`]), any link's target for the in-app or browser choice ([`link_target`])
//! and the shorthand references and ticket keys in plain text ([`references`]).

pub mod link_target;
pub mod pull_request_link;
pub mod references;
