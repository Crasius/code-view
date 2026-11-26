//! Videos in Markdown: an image whose address names a video file (`![demo](demo.webm)`, GitLab's
//! way), an HTML `<video>` or its `<source>` (GitHub, Azure DevOps), and a GitHub attachment
//! address alone in its paragraph, which GitHub shows as a player.

use super::{ImageSpec, Inline};

const EXTENSIONS: [&str; 7] = ["mp4", "m4v", "mov", "webm", "mkv", "ogv", "avi"];

/// Whether `src` names a video file: by its path's extension, or for attachments that keep the
/// name in the query (Azure DevOps' `fileName=`), by that name's.
pub(super) fn names_video(src: &str) -> bool {
    let (path, query) = src.split_once('?').unwrap_or((src, ""));
    let path = path.split('#').next().unwrap_or(path);
    has_video_extension(path)
        || query
            .split(['&', '#'])
            .filter_map(|pair| pair.split_once('='))
            .any(|(key, value)| key.eq_ignore_ascii_case("filename") && has_video_extension(value))
}

fn has_video_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// The video of a paragraph that is nothing but a GitHub attachment address: GitHub plays it as a
/// video (it attaches images as `<img>` tags).
pub(super) fn bare_attachment(inline: &Inline) -> Option<ImageSpec> {
    let mut shown =
        inline.runs.iter().filter(|run| run.style.image.is_some() || !run.text.trim().is_empty());
    let run = shown.next()?;
    if shown.next().is_some() || run.style.image.is_some() {
        return None;
    }
    let link = run.style.link.as_deref()?;
    (run.text.trim() == link && is_github_attachment(link)).then(|| ImageSpec {
        src: link.to_owned(),
        video: true,
        ..ImageSpec::default()
    })
}

/// `https://github.com/user-attachments/assets/<id>` and the older
/// `https://github.com/<owner>/<repository>/assets/<number>/<id>`.
fn is_github_attachment(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    path.starts_with("user-attachments/assets/") || path.split('/').nth(2) == Some("assets")
}

#[cfg(test)]
mod tests {
    use super::names_video;
    use crate::parse::{Block, parse};

    #[test]
    fn video_files_are_known_by_their_extension_or_attachment_name() {
        assert!(names_video("/uploads/abc/demo.WebM"));
        assert!(names_video("docs/intro.mp4?raw=true"));
        assert!(names_video(
            "https://dev.azure.com/o/p/_apis/wit/attachments/1?fileName=screen%20capture.mov"
        ));
        assert!(!names_video("shot.png"));
        assert!(!names_video("https://x.test/watch?v=mp4"));
    }

    #[test]
    fn a_github_attachment_alone_in_its_paragraph_is_a_video() {
        let video = |text: &str| match parse(text, None).first() {
            Some(Block::Paragraph(inline)) => inline
                .runs
                .iter()
                .find_map(|run| run.style.image.as_ref().filter(|image| image.video))
                .map(|image| image.src.clone()),
            _ => None,
        };
        let attachment = "https://github.com/user-attachments/assets/0f3b2c4e-1a2b-4c5d-9e8f";
        assert_eq!(video(attachment).as_deref(), Some(attachment));
        let older = "https://github.com/octo/app/assets/12/0f3b2c4e";
        assert_eq!(video(older).as_deref(), Some(older));
        assert_eq!(video(&format!("See {attachment}")), None, "a link inside a sentence stays");
        assert_eq!(video("https://github.com/octo/app/pull/3"), None);
        assert_eq!(video("<video src=\"demo.mp4\" controls></video>").as_deref(), Some("demo.mp4"));
        let source = "<video controls>\n<source src=\"a.webm\" type=\"video/webm\">\n</video>";
        assert_eq!(video(source).as_deref(), Some("a.webm"), "the source of a video");
    }
}
