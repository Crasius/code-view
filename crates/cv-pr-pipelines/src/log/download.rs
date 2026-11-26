//! Streams a log to disk with a live byte counter, then indexes it for the viewer. A running job's
//! log is fetched again from its start (no platform documents fetching only the rest) and only what
//! came after the known part is appended and indexed.
//!
//! A download writes `<name>.log.part`. Only a finished job's log that arrived whole is renamed to
//! `<name>.whole.log`, so that file is always complete: opening the finished job again (a moment
//! later or after a restart) indexes it from disk without a request. A running job's log stays in
//! the part file, as it is never whole; once the job finished, the next open downloads it once
//! more. The whole name differs from the plain `<name>.log` that earlier builds wrote while a job
//! ran or a download broke off, so such a file is never taken for a whole log; retention ages
//! every one of these files by the pull request's prefix.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use review_core::{Provider, ProviderError};
use review_store::LogFile;
use tokio::io::AsyncWrite;

const ERROR_MARKERS: [&str; 3] = ["##[error]", "error:", "error["];
/// The end of the known part that a fetch again must repeat, or the log was rewritten.
const CHECK_BYTES: u64 = 4 * 1024;

pub struct LoadedLog {
    pub file: LogFile,
    /// First `##[error]` / `error:` line, for "Jump to first error".
    pub first_error: Option<usize>,
}

/// A finished download: the log, or why the platform did not serve it, kept whole so the pane can
/// tell a log that is not there yet from a failure.
pub enum Fetched {
    Log(Arc<LoadedLog>),
    Failed(ProviderError),
}

impl Fetched {
    /// A finished job's whole log from disk when it is there, else downloaded.
    pub async fn of(request: DownloadRequest) -> Fetched {
        match open_or_download(request).await {
            Ok(log) => Fetched::Log(log),
            Err(error) => Fetched::Failed(error),
        }
    }
}

/// What fetching a running job's log again brought.
pub enum Grown {
    /// Nothing new.
    Same,
    /// New text after the known part, appended and indexed.
    Appended(Arc<LoadedLog>),
    /// The log no longer starts with the known part: it was downloaded anew.
    Replaced(Arc<LoadedLog>),
}

pub struct DownloadRequest {
    pub provider: Arc<dyn Provider>,
    pub repository: String,
    pub number: u64,
    pub log_id: String,
    /// The log's name in the cache; the part and the whole file are named after it.
    pub path: PathBuf,
    /// The job finished, so a download that arrives in full is the whole log.
    pub is_finished: bool,
    pub received: Arc<AtomicU64>,
    pub repaint: egui::Context,
}

/// The file a download writes, and a running job's log stays in.
pub fn part_file(path: &Path) -> PathBuf {
    path.with_extension("log.part")
}

/// A finished job's log that arrived whole.
pub fn whole_file(path: &Path) -> PathBuf {
    path.with_extension("whole.log")
}

async fn open_or_download(request: DownloadRequest) -> Result<Arc<LoadedLog>, ProviderError> {
    let whole = whole_file(&request.path);
    if request.is_finished && tokio::fs::try_exists(&whole).await? {
        return index(whole).await;
    }
    download(request).await
}

async fn download(request: DownloadRequest) -> Result<Arc<LoadedLog>, ProviderError> {
    if let Some(directory) = request.path.parent() {
        tokio::fs::create_dir_all(directory).await?;
    }
    let part = part_file(&request.path);
    let file = tokio::fs::File::create(&part).await?;
    let mut sink = CountingWriter {
        inner: file,
        received: request.received.clone(),
        repaint: request.repaint.clone(),
    };
    request
        .provider
        .download_log(&request.repository, request.number, &request.log_id, &mut sink)
        .await?;
    tokio::io::AsyncWriteExt::flush(&mut sink).await?;
    drop(sink);
    if !request.is_finished {
        return index(part).await;
    }
    let whole = whole_file(&request.path);
    tokio::fs::rename(&part, &whole).await?;
    index(whole).await
}

async fn index(path: PathBuf) -> Result<Arc<LoadedLog>, ProviderError> {
    let loaded = tokio::task::spawn_blocking(move || -> std::io::Result<LoadedLog> {
        let file = LogFile::open(&path)?;
        let first_error = file.first_match(&ERROR_MARKERS)?;
        Ok(LoadedLog { file, first_error })
    })
    .await
    .map_err(|error| ProviderError::Local(format!("indexing the log stopped: {error}")))??;
    Ok(Arc::new(loaded))
}

/// Fetches the log of `current` again and appends what follows the known part; a log that no
/// longer starts with it is downloaded anew.
pub async fn refresh(
    request: DownloadRequest,
    current: Arc<LoadedLog>,
) -> Result<Grown, ProviderError> {
    let known = current.file.byte_len();
    let check_start = known.saturating_sub(CHECK_BYTES);
    let path = current.file.path().to_owned();
    let check = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(&path)?;
        file.seek(SeekFrom::Start(check_start))?;
        let mut check = Vec::new();
        file.take(known - check_start).read_to_end(&mut check)?;
        Ok(check)
    })
    .await
    .map_err(|error| ProviderError::Local(format!("reading the log stopped: {error}")))??;
    let mut tail = Tail::new(known, check);
    let fetched = request
        .provider
        .download_log(&request.repository, request.number, &request.log_id, &mut tail)
        .await;
    if tail.is_rewritten() {
        return Ok(Grown::Replaced(download(request).await?));
    }
    fetched?;
    if tail.added.is_empty() {
        return Ok(Grown::Same);
    }
    let added = std::mem::take(&mut tail.added);
    let loaded = tokio::task::spawn_blocking(move || -> std::io::Result<LoadedLog> {
        std::io::Write::write_all(
            &mut std::fs::OpenOptions::new().append(true).open(current.file.path())?,
            &added,
        )?;
        let file = current.file.extended()?;
        let first_error = match current.first_error {
            Some(line) => Some(line),
            // The last known line may have gone on; it is searched again with the new ones.
            None => {
                file.first_match_from(&ERROR_MARKERS, current.file.line_count().saturating_sub(1))?
            }
        };
        Ok(LoadedLog { file, first_error })
    })
    .await
    .map_err(|error| ProviderError::Local(format!("indexing the log stopped: {error}")))??;
    Ok(Grown::Appended(Arc::new(loaded)))
}

/// Takes a log fetched again from its start: checks that it repeats the end of the known part and
/// keeps only what comes after it. Stops the stream at the first byte that differs.
struct Tail {
    known: u64,
    /// The last bytes of the known part, from `known - check.len()`.
    check: Vec<u8>,
    position: u64,
    added: Vec<u8>,
    differs: bool,
}

impl Tail {
    fn new(known: u64, check: Vec<u8>) -> Self {
        Self { known, check, position: 0, added: Vec::new(), differs: false }
    }

    /// False once the bytes differ from the known part.
    fn take(&mut self, bytes: &[u8]) -> bool {
        let start = self.position;
        let end = start + bytes.len() as u64;
        let check_start = self.known - self.check.len() as u64;
        let (from, to) = (start.max(check_start), end.min(self.known));
        if from < to
            && bytes[(from - start) as usize..(to - start) as usize]
                != self.check[(from - check_start) as usize..(to - check_start) as usize]
        {
            self.differs = true;
            return false;
        }
        if end > self.known {
            let skip = self.known.saturating_sub(start) as usize;
            self.added.extend_from_slice(&bytes[skip..]);
        }
        self.position = end;
        true
    }

    /// The fetched log differs from the known part or is shorter than it.
    fn is_rewritten(&self) -> bool {
        self.differs || self.position < self.known
    }
}

impl AsyncWrite for Tail {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.take(buffer) {
            Poll::Ready(Ok(buffer.len()))
        } else {
            Poll::Ready(Err(std::io::Error::other(
                "the log no longer starts with the part already shown",
            )))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Counts bytes for the progress line and wakes the UI at most every 64 KiB.
struct CountingWriter {
    inner: tokio::fs::File,
    received: Arc<AtomicU64>,
    repaint: egui::Context,
}

const REPAINT_EVERY: u64 = 64 * 1024;

impl AsyncWrite for CountingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(context, buffer);
        if let Poll::Ready(Ok(written)) = &poll {
            let before = self.received.fetch_add(*written as u64, Ordering::Relaxed);
            if before / REPAINT_EVERY != (before + *written as u64) / REPAINT_EVERY {
                self.repaint.request_repaint();
            }
        }
        poll
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests {
    use super::Tail;

    /// Feeds `log` in chunks of `size` bytes; `None` when it stopped at a difference.
    fn fetch(known: &[u8], log: &[u8], size: usize) -> Option<(Vec<u8>, bool)> {
        let check_from = known.len().saturating_sub(3);
        let mut tail = Tail::new(known.len() as u64, known[check_from..].to_vec());
        for chunk in log.chunks(size) {
            if !tail.take(chunk) {
                return None;
            }
        }
        Some((tail.added.clone(), tail.is_rewritten()))
    }

    #[test]
    fn keeps_only_what_follows_the_known_part() {
        let known = b"$ pytest\ncollec";
        let grown = b"$ pytest\ncollected 3 items\n";
        for size in [1, 2, 5, 9, 64] {
            assert_eq!(
                fetch(known, grown, size),
                Some((b"ted 3 items\n".to_vec(), false)),
                "chunks of {size}"
            );
        }
        assert_eq!(fetch(known, known, 4), Some((Vec::new(), false)), "nothing new");
        assert_eq!(
            fetch(b"", b"first\n", 4),
            Some((b"first\n".to_vec(), false)),
            "an empty log so far"
        );
    }

    #[test]
    fn notices_a_rewritten_or_shorter_log() {
        assert_eq!(
            fetch(b"$ pytest\ncollec", b"$ pytest\nrebuilt from scratch\n", 4),
            None,
            "differs where the known part ends"
        );
        assert_eq!(
            fetch(b"$ pytest\ncollec", b"$ pyt", 4),
            Some((Vec::new(), true)),
            "shorter than the known part"
        );
    }
}
