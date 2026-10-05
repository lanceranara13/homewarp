use std::pin::Pin;

use bollard::container::{AttachContainerResults, LogOutput};
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::Error;

/// A server that never ends its line must not be able to grow the buffer forever.
const LONGEST_LINE: usize = 64 * 1024;

/// A container's terminal: its output line by line, and a way to type into it.
pub struct Console {
    output: Pin<Box<dyn Stream<Item = Result<LogOutput, bollard::errors::Error>> + Send>>,
    input: Pin<Box<dyn AsyncWrite + Send>>,
    pending: Vec<u8>,
}

impl Console {
    pub(crate) fn new(attached: AttachContainerResults) -> Self {
        Self {
            output: attached.output,
            input: attached.input,
            pending: Vec::new(),
        }
    }

    /// The next line of output without its line ending, or `None` once the
    /// container has stopped writing.
    pub async fn next_line(&mut self) -> Result<Option<String>, Error> {
        loop {
            let newline = self.pending.iter().position(|byte| *byte == b'\n');
            if let Some(end) =
                newline.or((self.pending.len() >= LONGEST_LINE).then_some(LONGEST_LINE - 1))
            {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Ok(Some(text(&line)));
            }
            match self.output.next().await {
                Some(chunk) => self.pending.extend_from_slice(chunk?.as_ref()),
                None if self.pending.is_empty() => return Ok(None),
                None => return Ok(Some(text(&std::mem::take(&mut self.pending)))),
            }
        }
    }

    /// Types a command and presses enter.
    pub async fn send(&mut self, command: &str) -> Result<(), Error> {
        self.input.write_all(command.as_bytes()).await?;
        self.input.write_all(b"\n").await?;
        self.input.flush().await?;
        Ok(())
    }
}

fn text(line: &[u8]) -> String {
    String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

/// Removes terminal escape sequences, so that console output can be matched and
/// stored as plain text.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // A control sequence: parameters, then one final character from `@` to `~`.
            Some('[') => {
                chars.by_ref().find(|c| ('@'..='~').contains(c));
            }
            // An operating-system command, such as a window title: runs to a bell or to `ESC \`.
            Some(']') => {
                let mut previous = ']';
                for c in chars.by_ref() {
                    if c == '\u{7}' || (previous == '\u{1b}' && c == '\\') {
                        break;
                    }
                    previous = c;
                }
            }
            // Every other escape is two characters long.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn strips_colours_and_titles_and_keeps_the_text() {
        assert_eq!(
            strip_ansi("\u{1b}[32m[12:00:00 INFO]\u{1b}[0m: Done (9.1s)! For help"),
            "[12:00:00 INFO]: Done (9.1s)! For help"
        );
        assert_eq!(
            strip_ansi("\u{1b}]0;Paper\u{7}ready\u{1b}]0;x\u{1b}\\ now"),
            "ready now"
        );
        assert_eq!(strip_ansi("\u{1b}[?25lplain \u{1b}=text"), "plain text");
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
    }
}
