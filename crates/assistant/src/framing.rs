//! Shared framing for the Model Context Protocol's line protocol: one JSON-RPC message per
//! line, read within a byte limit. `rustgis-cli mcp`, `rustgis-cli mcp --attach` and the MCP
//! relay's socket connections all read through this, so the protocol parses one way
//! everywhere.
//!
//! The limit convention: the bytes read for one line — including its terminating `\n` — may
//! not exceed the limit. A last line at the end of the input carries no newline and may use
//! the whole limit: the limit always counts the line exactly as framed, in every case.

use std::io::{BufRead, Read};

/// The longest message read (a project's JSON can be large); longer lines are refused.
pub const MAX_LINE: u64 = 64 * 1024 * 1024;

/// What went wrong reading a message. [`Error::Io`] carries the reader's own text, which is
/// neutral (it names neither standard input nor a socket): the caller knows the source and
/// says it. The protocol complaints ([`Error::TooLong`], [`Error::NotUtf8]]) are about the
/// message itself and are sent to the client as they stand.
#[derive(Debug)]
pub enum Error {
    /// The reader failed.
    Io(std::io::Error),
    /// The line is longer than this many bytes (its `\n` included).
    TooLong(u64),
    /// The line is not UTF-8; a bad byte could change a path inside one, so it isn't guessed at.
    NotUtf8,
}

impl std::fmt::Display for Error {
    /// Renders the protocol complaints verbatim for the client; [`Error::Io`] keeps the
    /// reader's neutral text so the caller can name the source.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::TooLong(max) => write!(f, "message too long ({max} bytes at most)"),
            Error::NotUtf8 => write!(f, "message is not UTF-8"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// What to do with a line that passes the limit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Oversize {
    /// Skip the rest of the line, then report it: a long-lived reader (standard input) goes
    /// on to the next message without buffering the oversized line.
    Skip,
    /// Report at once and read nothing more: the caller closes the connection (a socket or a
    /// pre-authentication client), so no more than `max + 1` bytes are ever buffered.
    Stop,
}

/// Reads one message (one line) with [`read_message_max`] at [`MAX_LINE`].
pub fn read_message(input: &mut impl BufRead, oversize: Oversize) -> Result<Option<String>, Error> {
    read_message_max(input, MAX_LINE, oversize)
}

/// Reads one message (one line) from `input`, at most `max` bytes as framed above.
///
/// - `Ok(None)`: the end of the input, before any byte of a new message.
/// - `Ok(Some(message))`: one message, the line without its terminating `\n`.
/// - [`Oversize`] decides what an oversized line costs: `Skip` finishes skipping the line so
///   the reader recovers, `Stop` stops reading at once and the caller closes the connection.
pub fn read_message_max(input: &mut impl BufRead, max: u64, oversize: Oversize) -> Result<Option<String>, Error> {
    let mut buf = Vec::new();
    let n = input.by_ref().take(max.saturating_add(1)).read_until(b'\n', &mut buf).map_err(Error::Io)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.len() as u64 > max {
        // Over the limit whether the line ended or not (its newline is one of the bytes
        // counted). `Skip` finishes the line first, so the next message starts clean.
        if oversize == Oversize::Skip && buf.last() != Some(&b'\n') {
            loop {
                buf.clear();
                let m = input.by_ref().take(max).read_until(b'\n', &mut buf).map_err(Error::Io)?;
                if m == 0 || buf.last() == Some(&b'\n') {
                    break;
                }
            }
        }
        return Err(Error::TooLong(max));
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|_| Error::NotUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(r: &mut Cursor<&[u8]>, max: u64, oversize: Oversize) -> Result<Option<String>, String> {
        read_message_max(r, max, oversize).map_err(|e| e.to_string())
    }

    #[test]
    fn reads_messages_and_strips_the_newline() {
        let mut r = Cursor::new(&b"one\ntwo\r\n\n"[..]);
        assert_eq!(read(&mut r, 16, Oversize::Skip).ok().flatten().as_deref(), Some("one"));
        assert_eq!(read(&mut r, 16, Oversize::Skip).ok().flatten().as_deref(), Some("two\r"));
        assert_eq!(read(&mut r, 16, Oversize::Skip).ok().flatten().as_deref(), Some(""));
    }

    #[test]
    fn the_end_of_the_input_is_none_and_a_last_line_without_a_newline_is_a_message() {
        let mut empty = Cursor::new(&b""[..]);
        assert!(read(&mut empty, 16, Oversize::Skip).ok().flatten().is_none());
        let mut r = Cursor::new(&b"last line"[..]);
        assert_eq!(read(&mut r, 16, Oversize::Skip).ok().flatten().as_deref(), Some("last line"));
        assert!(read(&mut r, 16, Oversize::Skip).ok().flatten().is_none());
    }

    #[test]
    fn a_line_that_is_not_utf8_is_refused() {
        let mut r = Cursor::new(&b"{\"a\":\xff}\nnext\n"[..]);
        assert_eq!(read(&mut r, 16, Oversize::Skip).err().as_deref(), Some("message is not UTF-8"));
        // The line was read whole, so the next message is still framed correctly.
        assert_eq!(read(&mut r, 16, Oversize::Skip).ok().flatten().as_deref(), Some("next"));
    }

    #[test]
    fn the_limit_counts_the_newline_and_applies_at_every_boundary() {
        // max = 5: the line as read (its `\n` included) may be 5 bytes at most.
        let mut r = Cursor::new(&b"abcd\nabcde\nabcde"[..]);
        assert_eq!(read(&mut r, 5, Oversize::Skip).ok().flatten().as_deref(), Some("abcd"));
        assert_eq!(read(&mut r, 5, Oversize::Skip).err().as_deref(), Some("message too long (5 bytes at most)"));
        // A last line without a newline may use the whole limit — and one byte more is refused.
        assert_eq!(read(&mut r, 5, Oversize::Skip).ok().flatten().as_deref(), Some("abcde"));
        assert!(read(&mut r, 5, Oversize::Skip).ok().flatten().is_none());
        let mut r = Cursor::new(&b"abcdef"[..]);
        assert_eq!(read(&mut r, 5, Oversize::Skip).err().as_deref(), Some("message too long (5 bytes at most)"));
        // Exactly the limit with its newline is refused too: the newline is not a free byte.
        let mut r = Cursor::new(&b"abcde\n"[..]);
        assert_eq!(read(&mut r, 5, Oversize::Skip).err().as_deref(), Some("message too long (5 bytes at most)"));
    }

    #[test]
    fn skip_recovers_after_an_oversized_line_without_buffering_it_all() {
        let tail = b"\nnext\n";
        let mut data = vec![b'a'; 1 << 20];
        data.extend_from_slice(tail);
        let mut r = Cursor::new(data.as_slice());
        assert_eq!(read(&mut r, 5, Oversize::Skip).err().as_deref(), Some("message too long (5 bytes at most)"));
        assert_eq!(read(&mut r, 5, Oversize::Skip).ok().flatten().as_deref(), Some("next"));
        assert!(read(&mut r, 5, Oversize::Skip).ok().flatten().is_none());
    }

    #[test]
    fn stop_reports_at_once_and_reads_no_more() {
        let mut r = Cursor::new(&b"aaaaaXrest of the oversized line\nnext\n"[..]);
        assert_eq!(read(&mut r, 5, Oversize::Stop).err().as_deref(), Some("message too long (5 bytes at most)"));
        // `Stop` did not skip the rest of the line: the caller closes the connection instead.
        // The bounded read consumed the max bytes plus one overflow byte (the `X`) to detect the
        // oversized line, so what is left is exactly the remainder after that overflow byte.
        let mut left = String::new();
        r.read_to_string(&mut left).ok();
        assert_eq!(left, "rest of the oversized line\nnext\n");
    }

    #[test]
    fn an_io_failure_is_neutral_and_names_no_source() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset"))
            }
        }
        impl BufRead for Broken {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset"))
            }
            fn consume(&mut self, _: usize) {}
        }
        let why = read_message_max(&mut Broken, 5, Oversize::Stop).err().unwrap_or_else(|| panic!("expected an error"));
        assert_eq!(why.to_string(), "connection reset");
        assert!(!why.to_string().contains("standard input"), "the diagnostic names a source the helper cannot see: {why}");
    }
}
