//! Tern Surface Protocol v1 framing and single-owner terminal input.
//! Ported from OMP b91db66e packages/{wire/src/tsp,tui/src/native/encode,
//! tui/src/terminal}.ts. Copyright (c) 2026 Stencil Labs, Inc. MIT.
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    io,
    time::{Duration, Instant},
};
pub const VERSION: u64 = 1;
pub const DEFAULT_LIMIT: usize = 65_536;
pub const DEFAULT_CREDITS: usize = 2;
const PREFIX: &[u8] = b"\x1b_tsp;";
const OSC: &[u8] = b"\x1b]877;tsp;";

#[derive(Clone, Debug)]
pub struct Message {
    pub verb: char,
    pub value: Value,
}
#[derive(Debug)]
pub enum Packet {
    Event(Event),
    Tsp(Message),
    Da1,
}

fn parameter(segment: &str) -> Option<(&str, &str)> {
    let (key, value) = segment.split_once('=')?;
    (!key.is_empty()
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        && value
            .bytes()
            .all(|c| (0x21..=0x7e).contains(&c) && c != b';'))
    .then_some((key, value))
}
fn leads_parameter(body: &str, at: usize) -> bool {
    body.get(at..)
        .and_then(|tail| tail.split_once(';'))
        .and_then(|(head, _)| parameter(head))
        .is_some()
}
pub fn encode(verb: char, value: &Value, limit: usize, chunk_id: &mut u64) -> String {
    encode_body(verb, &value.to_string(), limit, chunk_id)
}
pub fn encode_body(verb: char, body: &str, limit: usize, chunk_id: &mut u64) -> String {
    let frame = |params: &str, text: &str| format!("\x1b_tsp;{verb}{params};{text}\x1b\\");
    if body.len() <= limit.max(1) {
        return frame("", body);
    }
    *chunk_id = chunk_id.wrapping_add(1);
    let id = *chunk_id;
    let mut at = 0;
    let mut out = String::new();
    while at < body.len() {
        let target = (at + limit.max(1)).min(body.len());
        let mut end = target;
        if end < body.len() {
            while end > at && (!body.is_char_boundary(end) || leads_parameter(body, end)) {
                end -= 1;
            }
            if end == at {
                end = target + 1;
                while end < body.len()
                    && (!body.is_char_boundary(end) || leads_parameter(body, end))
                {
                    end += 1;
                }
            }
        }
        let more = if end < body.len() { ";m=1" } else { "" };
        out.push_str(&frame(&format!(";c={id}{more}"), &body[at..end]));
        at = end;
    }
    out
}

#[derive(Default)]
pub struct Reader {
    chunks: HashMap<String, String>,
}
impl Reader {
    pub fn feed(&mut self, sequence: &[u8]) -> Option<Message> {
        let sequence = std::str::from_utf8(sequence).ok()?;
        let inner = sequence.strip_prefix("\x1b_tsp;")?.strip_suffix("\x1b\\")?;
        let (verb, mut body) = inner.split_once(';').unwrap_or((inner, ""));
        let verb = match verb {
            "r" => 'r',
            "e" => 'e',
            _ => return None,
        };
        let mut params = HashMap::new();
        while let Some((head, tail)) = body.split_once(';') {
            let Some((key, value)) = parameter(head) else {
                break;
            };
            params.insert(key.to_owned(), value.to_owned());
            body = tail;
        }
        let mut complete = body.to_owned();
        if let Some(id) = params.get("c") {
            let mut previous = self.chunks.remove(id).unwrap_or_default();
            previous.push_str(body);
            if params.get("m").is_some_and(|m| m == "1") {
                self.chunks.insert(id.clone(), previous);
                return None;
            }
            complete = previous;
        }
        let value: Value = serde_json::from_str(&complete).ok()?;
        let value = value.as_object()?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).is_some();
        let number = |key: &str| value.get(key).and_then(Value::as_f64).is_some();
        let boolean = |key: &str| value.get(key).and_then(Value::as_bool).is_some();
        let strings = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().all(Value::is_string))
        };
        let valid = if verb == 'r' {
            match value.get("r").and_then(Value::as_str) {
                Some("hello") => number("v") && text("term") && strings("kinds"),
                Some("blobs") => strings("have"),
                _ => false,
            }
        } else {
            match value.get("ev").and_then(Value::as_str) {
                Some("ack") => number("s"),
                Some("resize") => number("cols"),
                Some("theme") => boolean("dark"),
                Some("motion") => boolean("reduce"),
                Some("visible") => boolean("visible"),
                Some("toggle") => text("id") && boolean("collapsed"),
                Some("select" | "activate" | "change") => text("id") && text("item"),
                Some("action") => text("id") && text("act"),
                Some("edit") => {
                    text("id")
                        && text("text")
                        && ["from", "to", "cursor", "len"].iter().all(|k| number(k))
                }
                Some("undo" | "focus") => text("id"),
                Some("error") => text("msg"),
                Some("gone") => strings("ids"),
                Some("send") => {
                    ["sf", "id", "text"].iter().all(|key| text(key))
                        && value["sf"] != ""
                        && value["id"] != ""
                }
                _ => false,
            }
        };
        valid.then(|| Message {
            verb,
            value: Value::Object(value.clone()),
        })
    }
}

/// Framing precedes key/paste decoding. OSC 877 is normalized before Reader;
/// reports never enter the editable draft. Input split at any byte is retained.
#[derive(Default)]
pub struct InputDecoder {
    bytes: Vec<u8>,
    packets: VecDeque<Packet>,
    reader: Reader,
    paste: Option<Vec<u8>>,
    escape_since: Option<Instant>,
}
impl InputDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        self.decode();
    }
    pub fn pop(&mut self) -> Option<Packet> {
        self.packets.pop_front()
    }
    pub fn expired_escape(&mut self) {
        if self.bytes == b"\x1b"
            && self
                .escape_since
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(75))
        {
            self.bytes.clear();
            self.escape_since = None;
            self.key(KeyCode::Esc, KeyModifiers::NONE);
        }
    }
    // Native Unicode key records carry modifiers independently of the text.
    // Escape/control-string bytes still use the same protocol/paste decoder.
    pub(super) fn native_character(&mut self, ch: char, modifiers: KeyModifiers) {
        if modifiers.is_empty() || !self.bytes.is_empty() || self.paste.is_some() || ch == '\x1b' {
            let mut bytes=[0;4];self.push(ch.encode_utf8(&mut bytes).as_bytes());return;
        }
        let (code, modifiers)=match ch {
            '\r'|'\n'=>(KeyCode::Enter,modifiers),
            '\t'=>(KeyCode::Tab,modifiers),
            '\x08'|'\x7f'=>(KeyCode::Backspace,modifiers),
            '\x01'..='\x1a'=>(KeyCode::Char((b'a'+ch as u8-1) as char),modifiers|KeyModifiers::CONTROL),
            _=>(KeyCode::Char(ch),modifiers),
        };
        self.key(code,modifiers);
    }
    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.packets
            .push_back(Packet::Event(Event::Key(KeyEvent::new(code, modifiers))))
    }
    fn decode(&mut self) {
        loop {
            if self.bytes.is_empty() {
                self.escape_since = None;
                return;
            }
            if self.paste.is_some() {
                if let Some(end) = find(&self.bytes, b"\x1b[201~") {
                    let mut paste = self.paste.take().unwrap();
                    paste.extend(self.bytes.drain(..end));
                    self.bytes.drain(..6);
                    self.packets.push_back(Packet::Event(Event::Paste(
                        String::from_utf8_lossy(&paste).into_owned(),
                    )));
                    continue;
                }
                let keep = self.bytes.len().min(5);
                let drain = self.bytes.len() - keep;
                self.paste
                    .as_mut()
                    .unwrap()
                    .extend(self.bytes.drain(..drain));
                return;
            }
            if self.bytes[0] == 0x1b {
                self.escape_since.get_or_insert_with(Instant::now);
                if PREFIX.starts_with(&self.bytes) || OSC.starts_with(&self.bytes) {
                    return;
                }
                if self.bytes.starts_with(PREFIX) || self.bytes.starts_with(OSC) {
                    let osc = self.bytes.starts_with(OSC);
                    let offset = if osc { OSC.len() } else { PREFIX.len() };
                    let st = find(&self.bytes[offset..], b"\x1b\\").map(|at| (offset + at, 2));
                    let bel = osc
                        .then(|| {
                            self.bytes[offset..]
                                .iter()
                                .position(|c| *c == 7)
                                .map(|at| (offset + at, 1))
                        })
                        .flatten();
                    let ending = match (st, bel) {
                        (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
                        (a, b) => a.or(b),
                    };
                    let Some((end, len)) = ending else { return };
                    let mut frame = PREFIX.to_vec();
                    frame.extend_from_slice(&self.bytes[offset..end]);
                    frame.extend_from_slice(b"\x1b\\");
                    self.bytes.drain(..end + len);
                    if let Some(message) = self.reader.feed(&frame) {
                        self.packets.push_back(Packet::Tsp(message));
                    }
                    continue;
                }
                if self.bytes.starts_with(b"\x1b[200~") {
                    self.bytes.drain(..6);
                    self.paste = Some(Vec::new());
                    continue;
                }
                if self.bytes.len() == 1 {
                    return;
                }
                if self.bytes[1] == b'[' || self.bytes[1] == b'O' {
                    let Some(end) = self.bytes[2..]
                        .iter()
                        .position(|c| (0x40..=0x7e).contains(c))
                        .map(|i| i + 2)
                    else {
                        return;
                    };
                    let sequence = self.bytes.drain(..=end).collect::<Vec<_>>();
                    if sequence.starts_with(b"\x1b[?") && sequence.last() == Some(&b'c') {
                        self.packets.push_back(Packet::Da1);
                        continue;
                    }
                    let params =
                        std::str::from_utf8(&sequence[2..sequence.len() - 1]).unwrap_or("");
                    let modifiers =
                        match params.rsplit(';').next().and_then(|m| m.parse::<u8>().ok()) {
                            Some(2) => KeyModifiers::SHIFT,
                            Some(3) => KeyModifiers::ALT,
                            Some(5) => KeyModifiers::CONTROL,
                            Some(6) => KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                            _ => KeyModifiers::NONE,
                        };
                    let key = match *sequence.last().unwrap() {
                        b'A' => Some(KeyCode::Up),
                        b'B' => Some(KeyCode::Down),
                        b'C' => Some(KeyCode::Right),
                        b'D' => Some(KeyCode::Left),
                        b'H' => Some(KeyCode::Home),
                        b'F' => Some(KeyCode::End),
                        b'Z' => Some(KeyCode::BackTab),
                        b'~' => match params.split(';').next() {
                            Some("1" | "7") => Some(KeyCode::Home),
                            Some("3") => Some(KeyCode::Delete),
                            Some("4" | "8") => Some(KeyCode::End),
                            Some("5") => Some(KeyCode::PageUp),
                            Some("6") => Some(KeyCode::PageDown),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(key) = key {
                        self.key(key, modifiers)
                    }
                    continue;
                }
                // Non-TSP control strings are terminal reports, not draft text.
                if matches!(self.bytes[1], b']' | b'_' | b'P' | b'^') {
                    let Some(end) = find(&self.bytes, b"\x1b\\")
                        .map(|n| n + 2)
                        .or_else(|| self.bytes.iter().position(|c| *c == 7).map(|n| n + 1))
                    else {
                        return;
                    };
                    self.bytes.drain(..end);
                    continue;
                }
                if let Some((ch, len)) = first_char(&self.bytes[1..]) {
                    self.bytes.drain(..len + 1);
                    self.key(KeyCode::Char(ch), KeyModifiers::ALT);
                    continue;
                }
                return;
            }
            let byte = self.bytes[0];
            if byte < 0x20 || byte == 0x7f {
                self.bytes.remove(0);
                match byte {
                    b'\r' | b'\n' => self.key(KeyCode::Enter, KeyModifiers::NONE),
                    b'\t' => self.key(KeyCode::Tab, KeyModifiers::NONE),
                    8 | 127 => self.key(KeyCode::Backspace, KeyModifiers::NONE),
                    1..=26 => self.key(
                        KeyCode::Char((b'a' + byte - 1) as char),
                        KeyModifiers::CONTROL,
                    ),
                    _ => {}
                }
                continue;
            }
            if let Some((ch, len)) = first_char(&self.bytes) {
                self.bytes.drain(..len);
                self.key(KeyCode::Char(ch), KeyModifiers::NONE);
            } else {
                return;
            }
        }
    }
}
fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|part| part == needle)
}
fn first_char(bytes: &[u8]) -> Option<(char, usize)> {
    let width = match *bytes.first()? {
        0..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Some(('\u{fffd}', 1)),
    };
    if bytes.len() < width {
        return None;
    }
    Some((
        std::str::from_utf8(&bytes[..width])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('\u{fffd}'),
        width,
    ))
}

/// A platform poll, called only by TerminalDriver while it owns raw mode.
/// No crossterm reader/background input thread is started alongside it.
#[derive(Default)]
pub struct NativeInput {
    decoder: InputDecoder,
    #[cfg(windows)]
    utf16: Vec<(u16, KeyModifiers)>,
    #[cfg(windows)]
    original_mode: Option<u32>,
}
impl NativeInput {
    pub fn start(&mut self)->io::Result<()> {
        #[cfg(windows)] {use windows_sys::Win32::System::Console::*;let input=unsafe{GetStdHandle(STD_INPUT_HANDLE)};let mut mode=0;if unsafe{GetConsoleMode(input,&mut mode)}==0{return Err(io::Error::last_os_error())}if unsafe{SetConsoleMode(input,mode|ENABLE_VIRTUAL_TERMINAL_INPUT)}==0{return Err(io::Error::last_os_error())}self.original_mode=Some(mode);}
        Ok(())
    }
    pub fn stop(&mut self)->io::Result<()> {
        #[cfg(windows)] {use windows_sys::Win32::System::Console::*;if let Some(mode)=self.original_mode.take(){if unsafe{SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE),mode)}==0{return Err(io::Error::last_os_error())}}}
        Ok(())
    }
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Option<Packet>> {
        if let Some(packet) = self.decoder.pop() {
            return Ok(Some(packet));
        }
        self.decoder.expired_escape();
        self.read_platform(timeout)?;
        self.decoder.expired_escape();
        Ok(self.decoder.pop())
    }
    #[cfg(unix)]
    fn read_platform(&mut self, timeout: Duration) -> io::Result<()> {
        let mut fds = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut fds,
                1,
                timeout.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if ready == 0 {
            return Ok(());
        }
        let mut bytes = [0u8; 8192];
        let read =
            unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.decoder.push(&bytes[..read as usize]);
        Ok(())
    }
    #[cfg(windows)]
    fn read_platform(&mut self, timeout: Duration) -> io::Result<()> {
        use windows_sys::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::{Console::*, Threading::WaitForSingleObject},
        };
        let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        match unsafe {
            WaitForSingleObject(input, timeout.as_millis().min(u32::MAX as u128) as u32)
        } {
            WAIT_TIMEOUT => return Ok(()),
            WAIT_OBJECT_0 => {}
            _ => return Err(io::Error::last_os_error()),
        }
        let mut records: [INPUT_RECORD; 128] = unsafe { std::mem::zeroed() };
        let mut read = 0;
        if unsafe {
            ReadConsoleInputW(input, records.as_mut_ptr(), records.len() as u32, &mut read)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        for record in &records[..read as usize] {
            match record.EventType as u32 {
                KEY_EVENT => {
                    let key = unsafe { record.Event.KeyEvent };
                    if key.bKeyDown == 0 {
                        continue;
                    }
                    let ch = unsafe { key.uChar.UnicodeChar };
                    let modifiers =
                        if key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
                            KeyModifiers::CONTROL
                        } else {
                            KeyModifiers::NONE
                        } | if key.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
                            KeyModifiers::ALT
                        } else {
                            KeyModifiers::NONE
                        } | if key.dwControlKeyState & SHIFT_PRESSED != 0 {
                            KeyModifiers::SHIFT
                        } else {
                            KeyModifiers::NONE
                        };
                    if ch != 0 {
                        for _ in 0..key.wRepeatCount.max(1) {self.utf16.push((ch,modifiers));}
                        continue;
                    }
                    self.flush_utf16();
                    let code = match key.wVirtualKeyCode {
                        0x25 => Some(KeyCode::Left),
                        0x26 => Some(KeyCode::Up),
                        0x27 => Some(KeyCode::Right),
                        0x28 => Some(KeyCode::Down),
                        0x24 => Some(KeyCode::Home),
                        0x23 => Some(KeyCode::End),
                        0x2e => Some(KeyCode::Delete),
                        0x21 => Some(KeyCode::PageUp),
                        0x22 => Some(KeyCode::PageDown),
                        _ => None,
                    };
                    if let Some(code) = code {
                        self.decoder.key(code, modifiers)
                    }
                }
                WINDOW_BUFFER_SIZE_EVENT => {
                    self.flush_utf16();
                    let size = unsafe { record.Event.WindowBufferSizeEvent.dwSize };
                    self.decoder.packets.push_back(Packet::Event(Event::Resize(
                        size.X.max(1) as u16,
                        size.Y.max(1) as u16,
                    )));
                }
                _ => {}
            }
        }
        self.flush_utf16();
        Ok(())
    }
    #[cfg(windows)]
    fn flush_utf16(&mut self) {
        let keep = usize::from(
            self.utf16
                .last()
                .is_some_and(|(ch,_)| (0xd800..=0xdbff).contains(ch)),
        );
        let count = self.utf16.len() - keep;
        if count == 0 {
            return;
        }
        let units=self.utf16.drain(..count).collect::<Vec<_>>();let mut at=0;
        while at<units.len() {
            let (first,mut modifiers)=units[at];let paired=(0xd800..=0xdbff).contains(&first)&&units.get(at+1).is_some_and(|(next,_)|(0xdc00..=0xdfff).contains(next));
            let ch=if paired {modifiers|=units[at+1].1;let code=0x10000+(((first as u32-0xd800)<<10)|(units[at+1].0 as u32-0xdc00));char::from_u32(code).unwrap()}else{char::from_u32(first as u32).unwrap_or('\u{fffd}')};
            self.decoder.native_character(ch,modifiers);at+=if paired{2}else{1};
        }
    }
}
