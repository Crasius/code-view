use tokio::process::Command;

/// Child processes open no window here: nothing to hide.
pub(crate) fn hide_console(_command: &mut Command) {}
