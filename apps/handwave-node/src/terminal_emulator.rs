//! Headless terminal protocol endpoint. The browser only displays output;
//! terminal queries must receive live answers even when no browser is open.
use std::{
    io,
    time::{Duration, Instant},
};

const MAX_ROWS: u16 = 300;
const MAX_COLS: u16 = 500;
const MAX_REPLIES_PER_CHUNK: usize = 64;
const MAX_REPLY_BYTES_PER_CHUNK: usize = 1024;
const MAX_REPLIES_PER_SECOND: usize = 256;

struct Replies {
    pending: Vec<Vec<u8>>,
    bytes: usize,
    overflow: bool,
    window_start: Instant,
    window_count: usize,
}
impl Replies {
    fn push(&mut self, reply: Vec<u8>) {
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.window_count = 0;
        }
        if self.pending.len() >= MAX_REPLIES_PER_CHUNK
            || self.bytes + reply.len() > MAX_REPLY_BYTES_PER_CHUNK
            || self.window_count >= MAX_REPLIES_PER_SECOND
        {
            self.overflow = true;
            return;
        }
        self.bytes += reply.len();
        self.window_count += 1;
        self.pending.push(reply);
    }
}
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if i2.is_some() || params.len() != 1 || params[0].len() != 1 {
            return;
        }
        let parameter = params[0][0];
        match (i1, command, parameter) {
            (None, 'n', 5) => self.push(b"\x1b[0n".to_vec()),
            (private @ (None | Some(b'?')), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                let marker = if private.is_some() { "?" } else { "" };
                self.push(format!("\x1b[{marker}{};{}R", row + 1, col + 1).into_bytes());
            }
            // VT100 with advanced video; report only capabilities maintained
            // by the headless parser, rather than claiming a GUI terminal.
            (None, 'c', 0) => self.push(b"\x1b[?1;2c".to_vec()),
            (Some(b'>'), 'c', 0) => self.push(b"\x1b[>0;0;0c".to_vec()),
            _ => {}
        }
    }
    fn unhandled_escape(&mut self, _: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, byte: u8) {
        if i1.is_none() && i2.is_none() && byte == b'Z' {
            self.push(b"\x1b[?1;2c".to_vec());
        }
    }
}

pub struct TerminalEmulator {
    parser: vt100::Parser<Replies>,
}
impl TerminalEmulator {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows.clamp(1, MAX_ROWS),
                cols.clamp(1, MAX_COLS),
                0,
                Replies {
                    pending: Vec::new(),
                    bytes: 0,
                    overflow: false,
                    window_start: Instant::now(),
                    window_count: 0,
                },
            ),
        }
    }
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS));
    }
    /// Returns live protocol replies in their original order. vt100/vte invokes
    /// callbacks at each exact query position, including across chunk boundaries.
    /// Historical log replay must never be passed here and written to a child.
    pub fn process(&mut self, bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        self.parser.process(bytes);
        let replies = self.parser.callbacks_mut();
        if replies.overflow {
            replies.pending.clear();
            return Err(io::Error::other("terminal query response limit exceeded"));
        }
        replies.bytes = 0;
        Ok(std::mem::take(&mut replies.pending))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn answers_queries_at_each_cursor_position_and_preserves_split_sequences() {
        let mut terminal = TerminalEmulator::new(24, 80);
        assert_eq!(
            terminal.process(b"abc\x1b[6n\r\nxy\x1b[?6n").unwrap(),
            [b"\x1b[1;4R".to_vec(), b"\x1b[?2;3R".to_vec()]
        );
        assert!(terminal.process(b"\x1b[1").unwrap().is_empty());
        assert!(terminal.process(b"0;20H\x1b[").unwrap().is_empty());
        assert_eq!(terminal.process(b"6n").unwrap(), [b"\x1b[10;20R".to_vec()]);
    }
    #[test]
    fn reports_status_and_primary_secondary_device_attributes() {
        let mut terminal = TerminalEmulator::new(24, 80);
        assert_eq!(
            terminal.process(b"\x1b[5n\x1b[c\x1b[0c\x1b[>c\x1b[>0c\x1bZ").unwrap(),
            [
                b"\x1b[0n".to_vec(),
                b"\x1b[?1;2c".to_vec(),
                b"\x1b[?1;2c".to_vec(),
                b"\x1b[>0;0;0c".to_vec(),
                b"\x1b[>0;0;0c".to_vec(),
                b"\x1b[?1;2c".to_vec(),
            ]
        );
    }
    #[test]
    fn resize_unicode_cursor_moves_and_alternate_screen_use_real_screen_state() {
        let mut terminal = TerminalEmulator::new(24, 80);
        terminal.process("界a".as_bytes()).unwrap();
        assert_eq!(terminal.process(b"\x1b[6n").unwrap(), [b"\x1b[1;4R".to_vec()]);
        terminal.resize(37, 103);
        assert_eq!(terminal.process(b"\x1b[999;999H\x1b[6n").unwrap(), [b"\x1b[37;103R".to_vec()]);
        terminal.process(b"\x1b[?1049h\x1b[H").unwrap();
        assert_eq!(terminal.process(b"\x1b[6n").unwrap(), [b"\x1b[1;1R".to_vec()]);
    }
    #[test]
    fn osc_content_and_nonstandard_queries_do_not_generate_input() {
        let mut terminal = TerminalEmulator::new(24, 80);
        assert!(terminal.process(b"\x1b]0;title[6n\x07\x1b[4n\x1b[6;1n\x1b[6:1n\x1b[?5n").unwrap().is_empty());
    }
    #[test]
    fn bounded_responses_reject_query_floods() {
        let mut terminal = TerminalEmulator::new(24, 80);
        assert!(terminal.process(&b"\x1b[6n".repeat(65)).is_err());
        assert!(terminal.parser.callbacks().pending.is_empty());
        let mut terminal = TerminalEmulator::new(24, 80);
        for _ in 0..4 {
            assert_eq!(terminal.process(&b"\x1b[5n".repeat(64)).unwrap().len(), 64);
        }
        assert!(terminal.process(b"\x1b[5n").is_err());
    }
    #[test]
    fn screen_allocation_has_a_final_bound() {
        let mut terminal = TerminalEmulator::new(u16::MAX, u16::MAX);
        assert_eq!(terminal.parser.screen().size(), (MAX_ROWS, MAX_COLS));
        terminal.resize(0, 0);
        assert_eq!(terminal.parser.screen().size(), (1, 1));
    }
}
