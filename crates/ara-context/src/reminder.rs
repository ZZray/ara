//! Date/cwd reminder (OMP `session/date-cwd-reminder.ts`).
//!
//! The system prompt stays byte-stable so provider prefix caches survive; the
//! per-day/per-directory line rides on the first user turn at request time
//! and is never stored in the transcript. A changed reminder attaches to the
//! next new user turn (or a developer turn after the last message), leaving
//! every earlier request byte-identical.

use ara_ai::{Context, DeveloperMessage, Message, UserBlock, UserContent, UserMessage};
use std::collections::HashMap;
use std::sync::Mutex;

const TEMPLATE: &str = include_str!("../prompts/date-cwd-reminder.md");

/// `renderDateCwdReminder`.
pub fn render_date_cwd_reminder(date: &str, cwd: &str) -> String {
    let rendered =
        ara_prompt::render(TEMPLATE, &serde_json::json!({ "date": date, "cwd": cwd })).unwrap_or_else(|_| {
            format!("<system-reminder>\nToday: {date}; current working directory: '{cwd}'.\n</system-reminder>")
        });
    ara_prompt::js::trim(&rendered).to_string()
}

fn starts_with_reminder(message: &UserMessage, reminder: &str) -> bool {
    match &message.content {
        UserContent::Text(text) => text.starts_with(reminder),
        UserContent::Blocks(blocks) => matches!(blocks.first(), Some(UserBlock::Text(t)) if t.text == reminder),
    }
}

fn inject(message: &UserMessage, reminder: &str) -> UserMessage {
    let content = match &message.content {
        UserContent::Text(text) => UserContent::Text(format!("{reminder}\n\n{text}")),
        UserContent::Blocks(blocks) => {
            let mut out = vec![UserBlock::text(reminder)];
            out.extend(blocks.iter().cloned());
            UserContent::Blocks(out)
        }
    };
    UserMessage { content, ..message.clone() }
}

#[derive(Default)]
struct State {
    /// Index and original of the first user message the reminders belong to.
    root: Option<(usize, UserMessage)>,
    current: Option<String>,
    injections: HashMap<usize, Message>,
    controls: Vec<(usize, Message)>,
    /// Messages already seen by an earlier request (transcripts only grow).
    seen: usize,
}

/// `DateCwdReminderInjector`: keeps reminders append-only across requests.
#[derive(Default)]
pub struct DateCwdReminder {
    state: Mutex<State>,
}

impl DateCwdReminder {
    pub fn new() -> Self {
        DateCwdReminder::default()
    }

    /// Apply the current reminder, preserving earlier injected bytes.
    pub fn transform(&self, context: Context, date: &str, cwd: &str) -> Context {
        if context.system_prompt.is_empty() || context.messages.is_empty() {
            return context;
        }
        let Some((first_index, first_user)) = context.messages.iter().enumerate().find_map(|(i, m)| match m {
            Message::User(u) => Some((i, u.clone())),
            _ => None,
        }) else {
            return context;
        };
        let reminder = render_date_cwd_reminder(date, cwd);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let same_root = state.root.as_ref().is_some_and(|(i, u)| *i == first_index && *u == first_user);
        if !same_root {
            *state = State {
                root: Some((first_index, first_user.clone())),
                current: Some(reminder.clone()),
                ..State::default()
            };
            if !starts_with_reminder(&first_user, &reminder) {
                state.injections.insert(first_index, Message::User(inject(&first_user, &reminder)));
            }
        } else if state.current.as_deref() != Some(reminder.as_str()) {
            let seen = state.seen;
            let new_user = context.messages.iter().enumerate().rev().find_map(|(i, m)| match m {
                Message::User(u) if i >= seen => Some((i, u)),
                _ => None,
            });
            match new_user {
                Some((index, user)) => {
                    let injected = Message::User(inject(user, &reminder));
                    state.injections.insert(index, injected);
                }
                None => {
                    let anchor = context.messages.len() - 1;
                    let developer = Message::Developer(DeveloperMessage {
                        content: UserContent::Text(reminder.clone()),
                        timestamp: ara_ai::now_ms(),
                    });
                    state.controls.push((anchor, developer));
                }
            }
            state.current = Some(reminder);
        }
        let mut messages = Vec::with_capacity(context.messages.len() + state.controls.len());
        for (index, message) in context.messages.iter().enumerate() {
            messages.push(state.injections.get(&index).cloned().unwrap_or_else(|| message.clone()));
            messages.extend(state.controls.iter().filter(|(a, _)| *a == index).map(|(_, m)| m.clone()));
        }
        state.seen = context.messages.len();
        Context { messages, ..context }
    }
}
