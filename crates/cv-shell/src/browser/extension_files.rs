//! The extension's files, compiled in from `browser-extension/` so every build carries the
//! extension that matches it, and unpacked next to the host manifest for "Load unpacked".

use std::io;
use std::path::Path;

/// Path inside the extension folder and content, for every file the extension loads.
const FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../../../../browser-extension/manifest.json")),
    ("service_worker.js", include_bytes!("../../../../browser-extension/service_worker.js")),
    ("content.js", include_bytes!("../../../../browser-extension/content.js")),
    ("popup.html", include_bytes!("../../../../browser-extension/popup.html")),
    ("popup.css", include_bytes!("../../../../browser-extension/popup.css")),
    ("popup.js", include_bytes!("../../../../browser-extension/popup.js")),
    ("icons/icon-16.png", include_bytes!("../../../../browser-extension/icons/icon-16.png")),
    ("icons/icon-32.png", include_bytes!("../../../../browser-extension/icons/icon-32.png")),
    ("icons/icon-48.png", include_bytes!("../../../../browser-extension/icons/icon-48.png")),
    ("icons/icon-128.png", include_bytes!("../../../../browser-extension/icons/icon-128.png")),
];

/// Writes the extension into `folder`, replacing what an older build left there.
pub fn unpack(folder: &Path) -> io::Result<()> {
    for (name, bytes) in FILES {
        let path = folder.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::unpack;
    use crate::browser::EXTENSION_ID;
    use cv_testkit::ScratchDir;

    /// The unpacked folder loads: every file the manifest names is there, with the pinned key and
    /// only the permissions the README explains.
    #[test]
    fn unpacks_every_file_the_manifest_names() {
        let scratch = ScratchDir::new("extension-files");
        unpack(scratch.path()).expect("unpacks");
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(scratch.path().join("manifest.json")).expect("manifest"),
        )
        .expect("JSON");
        let mut named = vec![
            manifest["background"]["service_worker"].as_str().expect("worker").to_owned(),
            manifest["action"]["default_popup"].as_str().expect("popup").to_owned(),
        ];
        for script in manifest["content_scripts"][0]["js"].as_array().expect("scripts") {
            named.push(script.as_str().expect("script").to_owned());
        }
        for icon in manifest["icons"].as_object().expect("icons").values() {
            named.push(icon.as_str().expect("icon").to_owned());
        }
        named.extend(["popup.css".to_owned(), "popup.js".to_owned()]);
        for file in named {
            assert!(scratch.path().join(&file).is_file(), "{file} is unpacked");
        }
        assert_eq!(manifest["permissions"], serde_json::json!(["nativeMessaging", "storage"]));
        assert!(manifest["key"].as_str().is_some_and(|key| !key.is_empty()), "the id is pinned");
        assert!(manifest.get("host_permissions").is_none());
        let worker =
            std::fs::read_to_string(scratch.path().join("service_worker.js")).expect("worker");
        assert!(worker.contains("'de.crasius.codeview'"), "the worker asks our host");
        assert_eq!(EXTENSION_ID.len(), 32);
    }
}
