//! The browser extension's side in Code View (`browser-extension/`, `docs/browser-extension.md`):
//! the native messaging host the browser starts ([`host`], over `framing`), the answer it gives
//! (`link_check`), the pull requests the app itself opened in the browser ([`passes`]), the
//! registration with Chrome and Edge ([`registration`]) and the extension's files, unpacked for
//! "Load unpacked" ([`extension_files`]). The Settings row lives with Settings
//! (`cv-settings`, `settings/browser_extension/`).

pub mod extension_files;
pub(crate) mod framing;
pub mod host;
pub(crate) mod link_check;
pub mod passes;
mod platform;
pub mod registration;

use std::path::PathBuf;

use cv_services::connection::providers::ProviderFactory;
use review_store::AppPaths;

/// The host's name, as the extension and the browsers' registrations call it.
pub(crate) const HOST_NAME: &str = "de.crasius.codeview";
/// The extension's id, fixed by the public key in `browser-extension/manifest.json`.
pub(crate) const EXTENSION_ID: &str = "bjnmgiogdmaelabjgocfnplmmlbpfpog";
const _: () = assert!(EXTENSION_ID.len() == 32);

/// The origin the browser passes the host, the one `allowed_origins` lets in.
pub(crate) fn extension_origin() -> String {
    format!("chrome-extension://{EXTENSION_ID}/")
}

/// The unpacked extension, for "Load unpacked".
pub fn extension_dir(paths: &AppPaths) -> PathBuf {
    paths.browser_dir().join("extension")
}

/// The host manifest Windows registers for both browsers.
pub fn host_manifest(paths: &AppPaths) -> PathBuf {
    paths.browser_dir().join(format!("{HOST_NAME}.json"))
}

/// Serves the browser when it started this process as the native host; true when it did, and
/// the process should end without a window. `factory` builds the provider a `check` reads through.
pub fn serve_if_asked(factory: ProviderFactory) -> bool {
    let Some(origin) = origin_from_args(std::env::args()) else {
        return false;
    };
    let app = host::InstalledApp {
        paths: AppPaths::from_environment(),
        http: review_core::http::build_client(),
        factory,
    };
    // Fails only when the browser closed the pipes; nobody is left to tell, and the host has no
    // window or console to tell it in.
    let _browser_gone =
        host::serve(&origin, &mut std::io::stdin().lock(), &mut std::io::stdout().lock(), &app);
    true
}

/// The caller's origin when the browser started the host: `CodeView chrome-extension://<id>/
/// [--parent-window=<n>]`, or by hand `CodeView --native-messaging <origin>`.
fn origin_from_args(mut args: impl Iterator<Item = String>) -> Option<String> {
    let _program = args.next();
    let first = args.next()?;
    if first.starts_with("chrome-extension://") {
        return Some(first);
    }
    if first == "--native-messaging" {
        return args.next();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::origin_from_args;

    fn origin(args: &[&str]) -> Option<String> {
        origin_from_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn recognises_the_browser_start_and_the_manual_one() {
        let browser = [
            "CodeView.exe",
            "chrome-extension://bjnmgiogdmaelabjgocfnplmmlbpfpog/",
            "--parent-window=0",
        ];
        assert_eq!(
            origin(&browser).as_deref(),
            Some("chrome-extension://bjnmgiogdmaelabjgocfnplmmlbpfpog/")
        );
        let manual = ["CodeView", "--native-messaging", "chrome-extension://x/"];
        assert_eq!(origin(&manual).as_deref(), Some("chrome-extension://x/"));
        assert_eq!(
            origin(&["CodeView", "https://github.com/a/b/pull/1"]),
            None,
            "a link opens the app"
        );
        assert_eq!(origin(&["CodeView"]), None);
    }
}
