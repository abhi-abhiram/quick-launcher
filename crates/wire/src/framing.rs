//! JSON Lines framing (*Transport* in `docs/protocol.md`): one JSON object
//! per `\n`-terminated line, at most [`MAX_LINE`] bytes.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The longest line either side may send, not counting its `\n`.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    /// A line was longer than [`MAX_LINE`]. When reading, the rest of it has
    /// been skipped, so the next read starts at the following line.
    TooLong,
    /// A line wasn't valid JSON for the expected message.
    Json(serde_json::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::TooLong => write!(f, "line is longer than {MAX_LINE} bytes"),
            FrameError::Json(e) => write!(f, "invalid message: {e}"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameError::Io(e) => Some(e),
            FrameError::TooLong => None,
            FrameError::Json(e) => Some(e),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Reads one line into `buf`, without its `\n`. Returns `Ok(false)` at EOF
/// with nothing read; a last line without `\n` still counts.
///
/// A line over [`MAX_LINE`] is never held in memory: it's skipped up to its
/// `\n` and reported as [`FrameError::TooLong`].
pub fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Result<bool, FrameError> {
    buf.clear();
    let mut too_long = false;
    let mut read_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if available.is_empty() {
            break;
        }
        read_any = true;
        let (chunk, found_newline) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (&available[..i], true),
            None => (available, false),
        };
        if !too_long {
            if buf.len() + chunk.len() > MAX_LINE {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        let used = chunk.len() + usize::from(found_newline);
        reader.consume(used);
        if found_newline {
            break;
        }
    }
    if too_long {
        return Err(FrameError::TooLong);
    }
    Ok(read_any)
}

/// Reads the next message, skipping blank lines. Returns `Ok(None)` at EOF.
pub fn read_message<T: DeserializeOwned>(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
) -> Result<Option<T>, FrameError> {
    loop {
        if !read_line(reader, buf)? {
            return Ok(None);
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        return serde_json::from_slice(buf)
            .map(Some)
            .map_err(FrameError::Json);
    }
}

/// Writes `message` as one line and flushes. A message that would encode to
/// more than [`MAX_LINE`] bytes is not written.
pub fn write_message<T: Serialize>(writer: &mut impl Write, message: &T) -> Result<(), FrameError> {
    let mut line = serde_json::to_vec(message).map_err(FrameError::Json)?;
    if line.len() > MAX_LINE {
        return Err(FrameError::TooLong);
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()?;
    Ok(())
}
