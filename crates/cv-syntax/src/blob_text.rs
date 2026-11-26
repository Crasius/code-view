//! A file's text split into lines once: one buffer plus a 4-byte offset per line.

pub struct BlobText {
    text: String,
    /// Start of every line, followed by the end of the text.
    starts: Vec<u32>,
}

impl BlobText {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        let mut starts = Vec::with_capacity(text.len() / 32 + 2);
        starts.push(0);
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index as u32 + 1),
        );
        if !text.is_empty() && !text.ends_with('\n') {
            starts.push(text.len() as u32);
        }
        starts.shrink_to_fit();
        Self { text, starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len() - 1
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Line `index` (0-based) without its line break, and the first one without a byte order mark,
    /// like the text of a diff line.
    pub fn line(&self, index: usize) -> Option<&str> {
        let start = *self.starts.get(index)? as usize;
        let end = *self.starts.get(index + 1)? as usize;
        let line = &self.text[start..end];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = if index == 0 { line.strip_prefix('\u{FEFF}').unwrap_or(line) } else { line };
        Some(line)
    }

    pub fn heap_size(&self) -> usize {
        self.text.capacity() + self.starts.capacity() * std::mem::size_of::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use super::BlobText;

    #[test]
    fn splits_lines_like_git() {
        let text = BlobText::from_bytes(b"first\r\nsecond\n\nlast".to_vec());
        assert_eq!(text.line_count(), 4);
        let lines: Vec<&str> = (0..4).filter_map(|index| text.line(index)).collect();
        assert_eq!(lines, ["first", "second", "", "last"]);
        assert_eq!(text.line(4), None);
    }

    #[test]
    fn reads_the_first_line_without_its_byte_order_mark() {
        let text = BlobText::from_bytes("\u{FEFF}first\r\n\u{FEFF}second\n".as_bytes().to_vec());
        assert_eq!(text.line(0), Some("first"), "as the diff line reads it");
        assert_eq!(text.line(1), Some("\u{FEFF}second"), "only a file's start has a BOM");
    }

    #[test]
    fn trailing_newline_does_not_add_a_line() {
        assert_eq!(BlobText::from_bytes(b"a\nb\n".to_vec()).line_count(), 2);
        assert_eq!(BlobText::from_bytes(Vec::new()).line_count(), 0);
    }
}
