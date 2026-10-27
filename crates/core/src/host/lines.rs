//! Reading a plugin's stdout and stderr one line at a time, under the same
//! 4 MiB limit as `quick_launcher_wire::framing`, without blocking the
//! runtime.

use std::mem;

use quick_launcher_wire::framing::{FrameError, MAX_LINE};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Lines from `R`, each without its `\n`.
pub(crate) struct Lines<R> {
    reader: BufReader<R>,
    buf: Vec<u8>,
    /// The line being read is over the limit; the rest of it is skipped.
    too_long: bool,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    pub(crate) fn new(reader: R) -> Self {
        Lines {
            reader: BufReader::new(reader),
            buf: Vec::new(),
            too_long: false,
        }
    }

    /// The next line, or `None` at EOF. A last line without `\n` still
    /// counts. A line over [`MAX_LINE`] is never held in memory: it's
    /// skipped up to its `\n` and reported as [`FrameError::TooLong`].
    ///
    /// Cancel safe: what was read of a line is kept for the next call, so
    /// it can be raced against other futures in `select!`.
    pub(crate) async fn next(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return self.finish().map(|line| (!line.is_empty()).then_some(line));
            }
            let (chunk, found_newline) = match available.iter().position(|&b| b == b'\n') {
                Some(i) => (&available[..i], true),
                None => (available, false),
            };
            if !self.too_long {
                if self.buf.len() + chunk.len() > MAX_LINE {
                    self.too_long = true;
                    self.buf = Vec::new();
                } else {
                    self.buf.extend_from_slice(chunk);
                }
            }
            let used = chunk.len() + usize::from(found_newline);
            self.reader.consume(used);
            if found_newline {
                return self.finish().map(Some);
            }
        }
    }

    /// Ends the current line and starts the next.
    fn finish(&mut self) -> Result<Vec<u8>, FrameError> {
        if mem::take(&mut self.too_long) {
            return Err(FrameError::TooLong);
        }
        Ok(mem::take(&mut self.buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(input: &[u8]) -> Vec<Result<Vec<u8>, String>> {
        let mut lines = Lines::new(input);
        let mut out = Vec::new();
        loop {
            match lines.next().await {
                Ok(Some(line)) => out.push(Ok(line)),
                Ok(None) => return out,
                Err(e) => out.push(Err(e.to_string())),
            }
        }
    }

    #[tokio::test]
    async fn splits_lines_and_keeps_a_last_one_without_newline() {
        assert_eq!(
            read_all(b"one\n\ntwo\nthree").await,
            [
                Ok(b"one".to_vec()),
                Ok(vec![]),
                Ok(b"two".to_vec()),
                Ok(b"three".to_vec())
            ]
        );
    }

    #[tokio::test]
    async fn skips_a_line_over_the_limit_and_reads_the_next() {
        let mut input = vec![b'x'; MAX_LINE + 1];
        input.extend_from_slice(b"\nnext\n");
        let lines = read_all(&input).await;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].is_err());
        assert_eq!(lines[1], Ok(b"next".to_vec()));
    }

    #[tokio::test]
    async fn a_line_of_exactly_the_limit_is_kept() {
        let mut input = vec![b'x'; MAX_LINE];
        input.push(b'\n');
        let lines = read_all(&input).await;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].as_ref().map(Vec::len), Ok(MAX_LINE));
    }
}
