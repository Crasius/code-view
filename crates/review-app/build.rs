//! Embeds the Code View icon and version info into the Windows executable, so Explorer, the
//! taskbar, shortcuts and the file properties show it. `examples/render_icon.rs` renders the icon.
//! Builds for other targets embed nothing: the app bundle carries the icon on macOS. The check reads
//! the target, not the host, because the release build cross-compiles for Windows on Linux, with
//! `llvm-rc` (`docs/release.md`).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/codeview.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon/codeview.ico")
            .set("ProductName", "Code View")
            .set("FileDescription", "Code View \u{2013} pull request reviews")
            .set("InternalName", "CodeView")
            .set("OriginalFilename", "CodeView.exe");
        resource.compile().expect("embed the Windows icon resource");
    }
}
