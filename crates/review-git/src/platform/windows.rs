use tokio::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Starts `command` without a console window.
pub(crate) fn hide_console(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}
