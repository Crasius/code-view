//! Linux: the desktop's own fonts (the GNOME interface and monospace fonts, else fontconfig's
//! `sans-serif` and `monospace`, else Cantarell, Ubuntu, Noto Sans or DejaVu from the usual
//! folders), DejaVu Sans for symbols, the GNOME "Animations" setting, the XDG Downloads folder and
//! `xdg-open` for folders. The system draws the title bar (X11 or Wayland decorations) and places
//! the window; there is no Save As dialog.

use std::path::{Path, PathBuf};
use std::process::Command;

use egui::{Rangef, Rect, Ui, ViewportBuilder};
use review_store::checkouts::TerminalChoice;

pub(crate) use super::portable::register_chrome_fonts;
use super::portable::spawn_reaped;
pub use super::portable::{
    DialogOwner, FrameHold, PlacedWindow, dialog_owner, hide_console, hide_std_console,
};
use super::{FontFace, FontSources, SaveRequest, VariableUiFont};

pub const AZURE_CLI: &str = "az";
pub const HAS_SAVE_DIALOG: bool = false;
/// A clone's folder as Settings › Local checkouts shows it in its example.
pub const EXAMPLE_CLONE: &str = "/home/you/src/login-app";
/// The terminals Settings › Local checkouts offers, with their names.
pub const TERMINALS: &[(TerminalChoice, &str)] =
    &[(TerminalChoice::System, "The desktop's terminal")];
/// The bundle identifier on macOS, the Wayland app id and X11 window class here.
const APP_ID: &str = "de.crasius.codeview";

/// Where distributions put the fonts tried when fontconfig does not answer: Debian and Ubuntu
/// first, then Fedora, Arch and openSUSE.
const FONT_DIRS: [&str; 6] = [
    "/usr/share/fonts/truetype",
    "/usr/share/fonts/opentype",
    "/usr/share/fonts",
    "/usr/share/fonts/TTF",
    "/usr/share/fonts/OTF",
    "/usr/share/fonts/truetype/dejavu",
];
/// UI faces by preference: (regular, semibold) file names, searched in [`FONT_DIRS`] and one level
/// below (the per-family folders).
const UI_FACES: [(&str, &str); 4] = [
    ("Cantarell-VF.otf", "Cantarell-VF.otf"),
    ("Ubuntu-R.ttf", "Ubuntu-M.ttf"),
    ("NotoSans-Regular.ttf", "NotoSans-SemiBold.ttf"),
    ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf"),
];
const CODE_FACES: [&str; 4] = [
    "DejaVuSansMono.ttf",
    "NotoSansMono-Regular.ttf",
    "UbuntuMono-R.ttf",
    "LiberationMono-Regular.ttf",
];
/// Monochrome faces with wide symbol coverage (arrows, check marks, box drawing). Noto Color Emoji
/// holds bitmaps only, which egui does not draw; egui's bundled emoji fonts cover emoji.
const SYMBOL_FACES: [&str; 2] = ["DejaVuSans.ttf", "NotoSansSymbols2-Regular.ttf"];

/// The environment as started: apps started from the desktop see the session's `PATH`.
pub(crate) fn prepare_environment() {}

/// The system's window, named with the app's id: Wayland compositors and X11 docks match it to a
/// `de.crasius.codeview.desktop` file for the icon and the window grouping.
pub fn viewport(builder: ViewportBuilder) -> ViewportBuilder {
    builder.with_app_id(APP_ID)
}

/// The XDG Downloads folder (`XDG_DOWNLOAD_DIR` in `~/.config/user-dirs.dirs`, localized on many
/// desktops), else `~/Downloads`.
pub fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|config| config.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let user_dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    Some(user_dir(&user_dirs, "XDG_DOWNLOAD_DIR", &home).unwrap_or_else(|| home.join("Downloads")))
}

/// Opens `path` in the desktop's file manager; fails when `xdg-open` does not start.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    spawn_reaped(Command::new("xdg-open").arg(path))
}

/// Opens the desktop's default terminal (`x-terminal-emulator`) in `dir` running `program` with
/// `args`.
pub fn open_terminal(dir: &Path, program: &str, args: &[String]) -> std::io::Result<()> {
    spawn_reaped(
        Command::new("x-terminal-emulator").arg("-e").arg(program).args(args).current_dir(dir),
    )
}

/// The desktop's interface and monospace fonts as fontconfig resolves them, then the usual files.
pub(crate) fn font_sources() -> FontSources {
    let interface = gnome_font("font-name").unwrap_or_else(|| "sans-serif".to_owned());
    let monospace = gnome_font("monospace-font-name").unwrap_or_else(|| "monospace".to_owned());
    let regular = fc_match(&format!("{interface}:weight=regular"));
    let semibold = fc_match(&format!("{interface}:weight=semibold"));
    // One face for both weights is a variable font: weights come from its `wght` axis.
    let variable_ui = match (&regular, &semibold) {
        (Some(regular), Some(semibold))
            if regular.path == semibold.path && regular.index == semibold.index =>
        {
            Some(VariableUiFont { face: regular.clone(), optical_size: None })
        }
        _ => None,
    };
    let installed = |file: &str| find_font(file);
    let mut ui: Vec<FontFace> = regular.into_iter().collect();
    let mut bold: Vec<FontFace> = semibold.into_iter().collect();
    for (regular_file, semibold_file) in UI_FACES {
        ui.extend(installed(regular_file));
        bold.extend(installed(semibold_file));
    }
    let mut code: Vec<FontFace> =
        fc_match(&format!("{monospace}:weight=regular:spacing=mono")).into_iter().collect();
    code.extend(CODE_FACES.iter().filter_map(|file| installed(file)));
    FontSources {
        variable_ui,
        ui,
        semibold: bold,
        code,
        emoji: Vec::new(),
        symbol: SYMBOL_FACES.iter().filter_map(|file| installed(file)).collect(),
    }
}

/// GNOME's Settings → Accessibility → "Reduce Animation" (the interface's `enable-animations`
/// key) is on. Read once at start-up; desktops without the key animate.
pub(crate) fn prefers_reduced_motion() -> bool {
    gsettings("org.gnome.desktop.interface", "enable-animations")
        .is_some_and(|value| value == "false")
}

/// The system draws the title bar.
pub fn integrate_title_bar(_creation: &eframe::CreationContext<'_>) -> bool {
    false
}

/// The window's own parts of an integrated title bar: none, the system draws them.
pub struct Caption {
    bar: Rect,
}

impl Caption {
    pub fn show(_ui: &mut Ui, bar: Rect, _has_icon: bool) -> Self {
        Self { bar }
    }

    pub fn free(&self) -> Rangef {
        self.bar.x_range()
    }

    #[expect(
        clippy::unused_self,
        reason = "the same signature as Windows and macOS, where the bar publishes its caption"
    )]
    pub fn publish(self, _ctx: &egui::Context, _holes: &[Rect]) {}
}

/// winit reports no refresh rate the frame limit could use under every compositor.
pub fn monitor_refresh_hz(_frame: &eframe::Frame) -> Option<u32> {
    None
}

/// No dialog here ([`HAS_SAVE_DIALOG`] keeps the buttons away).
pub fn ask_save_path(_owner: DialogOwner, _request: &SaveRequest<'_>) -> Option<PathBuf> {
    None
}

/// A `gsettings` value without the quotes `GVariant` text puts around strings.
#[expect(
    clippy::disallowed_methods,
    reason = "read once at start-up, before the first frame: the fonts and the motion setting need it"
)]
fn gsettings(schema: &str, key: &str) -> Option<String> {
    let output = Command::new("gsettings").args(["get", schema, key]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().trim_matches('\'').to_owned())
}

/// The family of a GNOME font setting ("Cantarell 11" → "Cantarell").
fn gnome_font(key: &str) -> Option<String> {
    gsettings("org.gnome.desktop.interface", key).and_then(|value| font_family(&value))
}

/// The family of a Pango font description: everything before the size (and before style words
/// fontconfig takes as separate properties).
fn font_family(description: &str) -> Option<String> {
    const STYLES: [&str; 6] = ["Regular", "Medium", "Bold", "Italic", "Light", "Semi-Bold"];
    let mut words: Vec<&str> = description.split_whitespace().collect();
    if words.last().is_some_and(|word| word.parse::<f32>().is_ok()) {
        words.pop();
    }
    while words.len() > 1 && words.last().is_some_and(|word| STYLES.contains(word)) {
        words.pop();
    }
    let family = words.join(" ");
    (!family.is_empty()).then_some(family)
}

/// The font file fontconfig picks for `pattern`; only TrueType and OpenType files, which egui reads.
#[expect(
    clippy::disallowed_methods,
    reason = "read once at start-up, before the first frame: the fonts are set up from it"
)]
fn fc_match(pattern: &str) -> Option<FontFace> {
    let output =
        Command::new("fc-match").args(["--format=%{file}\n%{index}", pattern]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_match(&String::from_utf8(output.stdout).ok()?)
}

/// `file\nindex` from `fc-match`. The index's high 16 bits name a variable font's instance; egui
/// wants the face index alone.
fn parse_match(output: &str) -> Option<FontFace> {
    let (file, index) = output.split_once('\n')?;
    let path = PathBuf::from(file.trim());
    let is_readable =
        path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
            ["ttf", "otf", "ttc", "otc"].contains(&extension.to_ascii_lowercase().as_str())
        });
    let index: u32 = index.trim().parse().ok()?;
    is_readable.then_some(FontFace { path, index: index & 0xFFFF })
}

/// `file` in one of [`FONT_DIRS`] or in a family folder right below one.
fn find_font(file: &str) -> Option<FontFace> {
    let face = |path: PathBuf| path.is_file().then_some(FontFace { path, index: 0 });
    FONT_DIRS.iter().map(Path::new).find_map(|dir| {
        face(dir.join(file)).or_else(|| {
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .find_map(|entry| face(entry.path().join(file)))
        })
    })
}

/// The folder `key` names in a `user-dirs.dirs` file (`KEY="$HOME/Folder"`).
fn user_dir(user_dirs: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let value = user_dirs
        .lines()
        .find_map(|line| line.trim().strip_prefix(key)?.trim_start().strip_prefix('='))?;
    let value = value.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path != home).then_some(path)
}

/// Opens the desktop's default terminal in `dir`.
pub fn open_shell(dir: &Path, _terminal: TerminalChoice) -> std::io::Result<()> {
    spawn_reaped(Command::new("x-terminal-emulator").current_dir(dir))
}

/// The folder chooser of `zenity` (GNOME) or `kdialog` (KDE), whichever is installed; `None` when
/// cancelled or when neither is. Blocks, so it runs on a worker thread.
#[expect(
    clippy::disallowed_methods,
    reason = "the dialog blocks until the user decides; callers run it on a worker thread"
)]
pub fn ask_folder(_owner: DialogOwner, title: &str) -> Option<PathBuf> {
    let zenity = Command::new("zenity")
        .args(["--file-selection", "--directory", "--title"])
        .arg(title)
        .output();
    let output = match zenity {
        Ok(output) => output,
        Err(_missing) => Command::new("kdialog")
            .args(["--getexistingdirectory", ".", "--title"])
            .arg(title)
            .output()
            .ok()?,
    };
    if !output.status.success() {
        return None;
    }
    let chosen = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(chosen.trim_end_matches('\n'));
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{font_family, parse_match, user_dir};

    #[test]
    fn gnome_font_settings_name_the_family() {
        assert_eq!(font_family("Cantarell 11").as_deref(), Some("Cantarell"));
        assert_eq!(font_family("Ubuntu Sans Mono 13").as_deref(), Some("Ubuntu Sans Mono"));
        assert_eq!(font_family("Noto Sans Bold 10").as_deref(), Some("Noto Sans"));
        assert_eq!(font_family(""), None);
    }

    #[test]
    fn fc_match_output_keeps_the_face_index_of_readable_files() {
        let face = parse_match("/usr/share/fonts/cantarell/Cantarell-VF.otf\n393216").unwrap();
        assert_eq!(face.path, PathBuf::from("/usr/share/fonts/cantarell/Cantarell-VF.otf"));
        assert_eq!(face.index, 0, "the instance bits are dropped");
        assert_eq!(
            parse_match("/usr/share/fonts/noto/NotoSansCJK-Regular.ttc\n2").unwrap().index,
            2
        );
        assert!(
            parse_match("/usr/share/fonts/misc/6x13.pcf.gz\n0").is_none(),
            "bitmap fonts are not for egui"
        );
        assert!(parse_match("").is_none());
    }

    #[test]
    fn downloads_follow_the_xdg_user_dirs() {
        let home = Path::new("/home/ada");
        let dirs = "# written by xdg-user-dirs-update\nXDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads (privat)\"\n";
        assert_eq!(
            user_dir(dirs, "XDG_DOWNLOAD_DIR", home),
            Some(PathBuf::from("/home/ada/Downloads (privat)"))
        );
        assert_eq!(
            user_dir("XDG_DOWNLOAD_DIR=\"/data/incoming\"", "XDG_DOWNLOAD_DIR", home),
            Some(PathBuf::from("/data/incoming"))
        );
        // "$HOME/" alone means the folder is turned off.
        assert_eq!(user_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", "XDG_DOWNLOAD_DIR", home), None);
        assert_eq!(user_dir("", "XDG_DOWNLOAD_DIR", home), None);
    }
}
