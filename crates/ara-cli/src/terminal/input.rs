//! Host-owned terminal draft. Fixed TUI input focus and bracketed-paste owner.
//! OMP 596f2da (MIT; repository notices). Submission is still admitted by REPL.
use super::render_utils::{sanitize, wrap};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
#[derive(Default)]
pub struct InputBuffer {
    pub draft: String,
    pub cursor: usize,
    pub submitted: VecDeque<String>,
    pub result: Option<Result<Option<String>, String>>,
    history: Vec<String>,
    history_index: Option<usize>,
    saved: String,
    native_undo: Vec<(String,usize)>,
}
impl InputBuffer {
    pub(crate) fn native_edit(&mut self,from:usize,to:usize,text:&str,cursor:usize,len:usize) {
        if self.draft.encode_utf16().count()!=len || from>to{return}
        fn byte(text:&str,units:usize)->Option<usize>{let mut count=0;for(i,c)in text.char_indices(){if count==units{return Some(i)}count+=c.len_utf16();if count>units{return None}}(count==units).then_some(text.len())}
        let(Some(from),Some(to))=(byte(&self.draft,from),byte(&self.draft,to))else{return};
        let mut next=self.draft.clone();next.replace_range(from..to,&sanitize(&text.into()));
        let Some(cursor)=byte(&next,cursor)else{return};
        if next!=self.draft{self.native_undo.push((self.draft.clone(),self.cursor));self.history_index=None;}
        self.draft=next;self.cursor=cursor;
    }
    pub(crate) fn native_undo(&mut self) {if let Some((text,cursor))=self.native_undo.pop(){self.draft=text;self.cursor=cursor;self.history_index=None;}}
    pub(crate) fn native_send(&mut self,text:&str,prompt:bool) {
        // The owner routes an explicit send only from the current editor id.
        self.native_undo.push((self.draft.clone(),self.cursor));self.draft=sanitize(&text.into());self.cursor=self.draft.len();
        self.key(KeyEvent::new(KeyCode::Enter,KeyModifiers::NONE),prompt);
    }
    pub fn insert(&mut self, text: &str) {
        self.draft.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.history_index = None;
    }
    pub fn paste(&mut self, text: &str) {
        self.insert(&sanitize(&text.into()));
    }
    pub fn key(&mut self, key: KeyEvent, prompt: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl && prompt => self.result = Some(Err("terminal prompt interrupted".into())),
            KeyCode::Char('d') if ctrl && prompt && self.draft.is_empty() => self.result = Some(Ok(None)),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.draft.len(),
            KeyCode::Char('u') if ctrl => {
                self.draft.replace_range(..self.cursor, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.draft.truncate(self.cursor),
            KeyCode::Char('w') if ctrl => {
                let start = word_left(&self.draft, self.cursor);
                self.draft.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Left => {
                self.cursor =
                    if ctrl || alt { word_left(&self.draft, self.cursor) } else { previous(&self.draft, self.cursor) }
            }
            KeyCode::Right => {
                self.cursor =
                    if ctrl || alt { word_right(&self.draft, self.cursor) } else { next(&self.draft, self.cursor) }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.draft.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let start = if alt { word_left(&self.draft, self.cursor) } else { previous(&self.draft, self.cursor) };
                self.draft.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            KeyCode::Delete => {
                let end = next(&self.draft, self.cursor);
                self.draft.replace_range(self.cursor..end, "");
            }
            KeyCode::Char('d') if ctrl => {
                let end = next(&self.draft, self.cursor);
                self.draft.replace_range(self.cursor..end, "");
            }
            KeyCode::Up if !self.history.is_empty() => {
                if self.history_index.is_none() {
                    self.saved = self.draft.clone();
                }
                let index = self.history_index.unwrap_or(self.history.len()).saturating_sub(1);
                self.history_index = Some(index);
                self.draft = self.history[index].clone();
                self.cursor = self.draft.len();
            }
            KeyCode::Down if self.history_index.is_some() => {
                let index = self.history_index.unwrap() + 1;
                if index < self.history.len() {
                    self.history_index = Some(index);
                    self.draft = self.history[index].clone();
                } else {
                    self.history_index = None;
                    self.draft = self.saved.clone();
                }
                self.cursor = self.draft.len();
            }
            KeyCode::Enter if alt || key.modifiers.contains(KeyModifiers::SHIFT) => self.insert("\n"),
            KeyCode::Enter => {
                let input = std::mem::take(&mut self.draft);
                self.cursor = 0;
                self.history_index = None;
                if !input.is_empty() && self.history.last() != Some(&input) {
                    self.history.push(input.clone());
                }
                let submitted = format!("{input}\n");
                if prompt {
                    self.result = Some(Ok(Some(submitted)));
                } else {
                    self.submitted.push_back(submitted);
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => self.insert(&c.to_string()),
            KeyCode::Tab => self.insert("\t"),
            _ => {}
        }
    }
    /// Caret is a display marker in the owned draft; no terminal reader or
    /// drawing callback accesses the Core/native argument Graph.
    pub fn rows(&self, columns: usize, maximum: usize) -> Vec<String> {
        let head = &self.draft[..self.cursor];
        let tail = &self.draft[self.cursor..];
        let (caret, rest) = if let Some(character) = tail.chars().next() {
            (character.to_string(), &tail[character.len_utf8()..])
        } else {
            (" ".to_owned(), "")
        };
        let content = format!("> {head}\x1b[7m{caret}\x1b[27m{rest}");
        let rows = content.split('\n').flat_map(|line| wrap(line, columns)).collect::<Vec<_>>();
        if rows.len() > maximum { rows[rows.len() - maximum..].to_vec() } else { rows }
    }
}
fn previous(text: &str, cursor: usize) -> usize {
    let mut offset = 0;
    let mut previous = 0;
    for grapheme in super::unicode::graphemes_str(text) {
        if offset >= cursor {
            break;
        }
        previous = offset;
        offset += grapheme.len();
    }
    previous
}
fn next(text: &str, cursor: usize) -> usize {
    let mut offset = 0;
    for grapheme in super::unicode::graphemes_str(text) {
        offset += grapheme.len();
        if offset > cursor {
            return offset;
        }
    }
    text.len()
}
fn word_left(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .trim_end()
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}
fn word_right(text: &str, cursor: usize) -> usize {
    let suffix = &text[cursor..];
    let start = suffix.len() - suffix.trim_start().len();
    suffix[start..]
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, _)| cursor + start + i)
        .unwrap_or(text.len())
}
