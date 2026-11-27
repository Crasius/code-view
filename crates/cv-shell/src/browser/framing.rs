//! Chrome's native messaging framing: each message is UTF-8 JSON preceded by its byte length as a
//! 32-bit integer in native byte order, on stdin from the browser and on stdout back.

// RS-DONT-04: reads lengths and bytes the browser (or anyone running the host by hand) sends.
#![warn(clippy::string_slice, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use std::io::{self, Read, Write};

/// The longest request the host reads: one carries a URL. Our own limit, far below the
/// browser's 64 MiB.
const MAX_REQUEST: u32 = 64 * 1024;
/// The longest message the browser accepts from a host (1 MiB, Chrome's native messaging docs).
const MAX_ANSWER: usize = 1024 * 1024;

/// The next message's bytes, or `None` once the browser closed stdin between messages.
pub fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0_u8; 4];
    if !fill_or_end(input, &mut length)? {
        return Ok(None);
    }
    let length = u32::from_ne_bytes(length);
    if length > MAX_REQUEST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a request of {length} bytes is over the host's limit of {MAX_REQUEST}"),
        ));
    }
    let mut body = vec![0_u8; usize::try_from(length).map_err(io::Error::other)?];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Writes `json` as one message and flushes it, so the browser gets it at once.
pub fn write_message(output: &mut impl Write, json: &[u8]) -> io::Result<()> {
    if json.len() > MAX_ANSWER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an answer over the browser's 1 MiB",
        ));
    }
    let length = u32::try_from(json.len()).map_err(io::Error::other)?;
    output.write_all(&length.to_ne_bytes())?;
    output.write_all(json)?;
    output.flush()
}

/// Fills `buffer`; false when the stream ended before its first byte, an error when it ended
/// inside it.
fn fill_or_end(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while let Some(rest) = buffer.get_mut(filled..)
        && !rest.is_empty()
    {
        match input.read(rest) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "a cut message length"));
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use super::{MAX_REQUEST, read_message, write_message};

    fn framed(json: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_message(&mut bytes, json.as_bytes()).expect("writes");
        bytes
    }

    #[test]
    fn a_message_round_trips_with_a_native_endian_length() {
        let json = r#"{"type":"check","url":"https://example.com/größe"}"#;
        let bytes = framed(json);
        let length = u32::try_from(json.len()).expect("short");
        assert_eq!(bytes.get(..4), Some(length.to_ne_bytes().as_slice()));
        let mut input = Cursor::new(bytes);
        assert_eq!(read_message(&mut input).expect("reads"), Some(json.as_bytes().to_vec()));
        assert_eq!(
            read_message(&mut input).expect("ends cleanly"),
            None,
            "stdin closed between messages"
        );
    }

    #[test]
    fn refuses_cut_and_oversized_messages() {
        let mut cut = Cursor::new(vec![10, 0]);
        assert_eq!(
            read_message(&mut cut).expect_err("cut length").kind(),
            ErrorKind::UnexpectedEof
        );

        let mut short_body = framed(r#"{"type":"ping"}"#);
        short_body.truncate(8);
        let error = read_message(&mut Cursor::new(short_body)).expect_err("cut body");
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);

        let mut huge = (MAX_REQUEST + 1).to_ne_bytes().to_vec();
        huge.extend([b' '; 16]);
        let error = read_message(&mut Cursor::new(huge)).expect_err("too long");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }
}
