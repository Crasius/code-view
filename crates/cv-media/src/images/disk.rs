//! Remote image bodies kept on disk, one file per image, so an image seen once never downloads
//! again while the host says it is unchanged.
//!
//! A kept body stays past the lifetime its host named: once stale it is still shown at once and
//! asked about with its validators behind it ([`super::ImageStore`]), and a `304` only renews the
//! lifetime. Hosts that say `no-cache` or `no-store` get a copy that is stale from the start, so
//! every load asks; a host without validators answers that question with the whole body. The
//! retention sweep removes a file 14 days after it was last read (`review_store::retention`), so
//! every read touches the file's modification time.
//!
//! The file: [`MAGIC`], the expiry (seconds since the epoch, little endian), the `ETag`, the
//! `Last-Modified` and the `Content-Type` (each a little endian `u16` length, then its UTF-8
//! bytes; empty when the host sent none), then the body. A file in another shape counts as no
//! copy and is replaced by the next download.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write as _};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use review_core::http::image::Validators;

/// How long a remote image counts as current when its host does not say.
const DEFAULT_MAX_AGE: Duration = Duration::from_hours(24);
const MAGIC: &[u8; 4] = b"CVI2";
/// Where the expiry starts: right after [`MAGIC`].
const EXPIRY_AT: u64 = 4;

/// A body read back from disk.
pub struct Kept {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub validators: Validators,
    /// Within the lifetime its host named: shown without asking.
    pub is_fresh: bool,
}

/// The body kept at `path`, fresh or stale; `None` when there is none or it cannot be read.
pub fn read(path: &Path) -> Option<Kept> {
    let file = std::fs::read(path).ok()?;
    let (magic, rest) = file.split_first_chunk::<4>()?;
    if magic != MAGIC {
        return None;
    }
    let (expiry, rest) = take_u64(rest)?;
    let (etag, rest) = take_text(rest)?;
    let (last_modified, rest) = take_text(rest)?;
    let (content_type, body) = take_text(rest)?;
    touch(path);
    Some(Kept {
        bytes: body.to_vec(),
        content_type,
        validators: Validators { etag, last_modified },
        is_fresh: expiry > now_secs(),
    })
}

/// Keeps `bytes` at `path` for `max_age` (none: [`DEFAULT_MAX_AGE`]); a zero age keeps a copy that
/// is asked about on every load.
pub fn write(
    path: &Path,
    bytes: &[u8],
    content_type: Option<&str>,
    validators: &Validators,
    max_age: Option<Duration>,
) {
    let texts = [validators.etag.as_deref(), validators.last_modified.as_deref(), content_type];
    let mut file = Vec::with_capacity(bytes.len() + 64);
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&expiry(max_age).to_le_bytes());
    for text in texts {
        // A header longer than a u16 counts as not sent: no host sends one that long.
        let text = text.unwrap_or_default();
        let (length, text) = u16::try_from(text.len()).map_or((0, ""), |length| (length, text));
        file.extend_from_slice(&length.to_le_bytes());
        file.extend_from_slice(text.as_bytes());
    }
    file.extend_from_slice(bytes);
    // A cache that cannot be written only means the image downloads again next time.
    let _uncached = write_whole(path, &file);
}

/// Writes `bytes` to `path` whole or not at all: into a `.part` file beside it, then renamed.
pub(super) fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = path.with_extension("part");
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, path))
}

/// The host said the body at `path` is unchanged: it counts as current for `max_age` again.
pub fn renew(path: &Path, max_age: Option<Duration>) {
    // A lifetime that cannot be written only means the next load asks again.
    let _asks_again = File::options().write(true).open(path).and_then(|mut file| {
        file.seek(SeekFrom::Start(EXPIRY_AT))?;
        file.write_all(&expiry(max_age).to_le_bytes())
    });
}

/// Lets go of a body that does not decode; one that cannot be removed fails the same way next
/// time, which is what the card says.
pub fn remove(path: &Path) {
    let _kept = std::fs::remove_file(path);
}

/// Marks the file as read now, for the retention sweep.
pub(super) fn touch(path: &Path) {
    // A time that cannot be set only lets the sweep remove the file sooner.
    let _sooner = File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()));
}

fn expiry(max_age: Option<Duration>) -> u64 {
    now_secs().saturating_add(max_age.unwrap_or(DEFAULT_MAX_AGE).as_secs())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

fn take_u64(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let (head, rest) = bytes.split_first_chunk::<8>()?;
    Some((u64::from_le_bytes(*head), rest))
}

fn take_text(bytes: &[u8]) -> Option<(Option<String>, &[u8])> {
    let (length, rest) = bytes.split_first_chunk::<2>()?;
    let (text, rest) = rest.split_at_checked(usize::from(u16::from_le_bytes(*length)))?;
    let text = std::str::from_utf8(text).ok()?;
    Some(((!text.is_empty()).then(|| text.to_owned()), rest))
}

#[cfg(test)]
mod tests {
    use cv_testkit::ScratchDir;

    use super::*;

    fn validators() -> Validators {
        Validators {
            etag: Some("\"v1\"".into()),
            last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
        }
    }

    #[test]
    fn keeps_the_body_with_its_validators_and_type() {
        let folder = ScratchDir::new("image-disk-keeps");
        let path = folder.path().join("connection").join("0123");
        write(&path, b"png", Some("image/png"), &validators(), None);
        let kept = read(&path).expect("kept");
        assert_eq!(kept.bytes, b"png");
        assert_eq!(kept.content_type.as_deref(), Some("image/png"));
        assert_eq!(kept.validators, validators());
        assert!(kept.is_fresh, "a day when the host does not say");
    }

    /// `no-cache` hosts still get a copy, asked about on every load; a 304 renews it.
    #[test]
    fn a_stale_copy_stays_until_renewed() {
        let folder = ScratchDir::new("image-disk-stale");
        let path = folder.path().join("a");
        write(&path, b"png", None, &Validators::NONE, Some(Duration::ZERO));
        let kept = read(&path).expect("a stale copy is still kept");
        assert!(!kept.is_fresh);
        assert_eq!((kept.content_type, kept.validators), (None, Validators::NONE));
        renew(&path, Some(Duration::from_secs(600)));
        let renewed = read(&path).expect("kept");
        assert!(renewed.is_fresh);
        assert_eq!(renewed.bytes, b"png", "renewing leaves the body alone");
    }

    /// The retention sweep counts from the last read, not from the download.
    #[test]
    fn reading_marks_the_file_as_used() {
        let folder = ScratchDir::new("image-disk-touch");
        let path = folder.path().join("a");
        write(&path, b"png", None, &Validators::NONE, None);
        let month_ago = SystemTime::now() - Duration::from_hours(24 * 30);
        File::options().write(true).open(&path).unwrap().set_modified(month_ago).unwrap();
        read(&path).expect("kept");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(modified.elapsed().unwrap_or_default() < Duration::from_hours(1));
    }

    #[test]
    fn files_of_another_shape_are_no_copy() {
        let folder = ScratchDir::new("image-disk-shape");
        let path = folder.path().join("a");
        // The shape before validators were kept: the expiry, then the body.
        let mut old = u64::MAX.to_le_bytes().to_vec();
        old.extend_from_slice(b"png");
        std::fs::write(&path, old).unwrap();
        assert!(read(&path).is_none());
        std::fs::write(&path, b"CVI2").unwrap();
        assert!(read(&path).is_none(), "cut short");
    }
}
