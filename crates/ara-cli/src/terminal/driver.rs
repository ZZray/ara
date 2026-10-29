//! Host-owned fixed terminal lifecycle, one output owner and 80ms scheduler.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::{
    evaluation::{Control, DisplayPort},
    input::InputBuffer,
    markdown_capabilities::{MarkdownCapabilities, resolve_markdown_capabilities},
    theme::Theme,
    transcript::{FramePlan, HistoryKind},
    types::FrameContext,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
static ACTIVE: OnceLock<Mutex<Option<Weak<Shared>>>> = OnceLock::new();
struct Shared {
    state: Mutex<State>,
    prompt_changed: Condvar,
    quitting: AtomicBool,
    display: Arc<DisplayPort>,
}
struct State {
    owned: FramePlan,
    generation: u64,
    history_ack: Option<HistoryWriteReceipt>,
    frame: FrameContext,
    painted: Vec<String>,
    spinner: usize,
    spinner_active: bool,
    dirty: bool,
    raw: bool,
    active_generation: Option<u64>,
    cancel: Option<CancellationToken>,
    write_failed: bool,
    prompt: bool,
    prompt_cancelled: bool,
    prompt_ticket: Option<Weak<PromptReadTicket>>,
    input: InputBuffer,
    download_events: crate::download_activity::DownloadSubscription,
    download_hud: super::download_hud::DownloadHud,
    native_input: super::tsp_wire::NativeInput,
    native: super::tsp_backend::Backend,
    native_surface: super::tsp_backend::Surface,
}
/// One blocking prompt reader's passive suspension ticket. It cannot cancel
/// a future reader, a Run or an already completed Enter/EOF result.
#[derive(Default)]
pub struct PromptReadTicket {
    suspended: AtomicBool,
}
pub const PASSIVE_PROMPT_SUSPENDED: &str = "terminal prompt suspended for late diagnostics";

#[derive(Clone, Copy, Debug)]
pub struct HistoryWriteReceipt {
    pub id: u64,
    pub kind: HistoryKind,
}
pub struct TerminalDriver {
    shared: Arc<Shared>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    theme: Arc<Theme>,
}
impl TerminalDriver {
    pub fn for_stdio(mode: ara_edit::EditMode) -> Option<Arc<Self>> {
        let is_tty = io::stdout().is_terminal() && io::stdin().is_terminal();
        if !is_tty {
            return None;
        }
        let capabilities = resolve_markdown_capabilities(
            &|name| std::env::var(name).ok(),
            match std::env::consts::OS {
                "windows" => "win32",
                "macos" => "darwin",
                platform => platform,
            },
            is_tty,
        );
        Self::for_stdio_with_capabilities(mode, capabilities)
    }
    /// Hosts select capabilities explicitly; renderers do not infer them.
    /// The existing frame remains the sole effective hyperlink owner.
    pub fn for_stdio_with_capabilities(
        _mode: ara_edit::EditMode,
        capabilities: MarkdownCapabilities,
    ) -> Option<Arc<Self>> {
        if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
            return None;
        }
        let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 30));
        let hyperlinks = std::env::var_os("NO_COLOR").is_none()
            && (std::env::var_os("WT_SESSION").is_some() || std::env::var_os("TERM_PROGRAM").is_some());
        let theme = Arc::new(if std::env::var("ARA_TERMINAL_THEME").ok().as_deref() == Some("light") {
            Theme::light()
        } else {
            Theme::dark()
        },
        );
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                owned: FramePlan { history: None, viewport: Vec::new(),
                },
                generation: 0,
                history_ack: None,
                frame: FrameContext {
                    columns: columns as usize,
                    rows: rows as usize,
                    width_epoch: 0,
                    hyperlinks,
                    true_color: capabilities.true_color,
                    text_sizing: capabilities.text_sizing,
                    image_protocol: capabilities.image_protocol,
                },
                painted: Vec::new(),
                spinner: 0,
                spinner_active: false,
                dirty: false,
                raw: false,
                active_generation: None,
                cancel: None,
                write_failed: false,
                prompt: false,
                prompt_cancelled: false,
                prompt_ticket: None,
                input: InputBuffer::default(),
                download_events: crate::download_activity::subscribe(),
                download_hud: Default::default(),
                native_input: Default::default(),
                native: Default::default(),
                native_surface: Default::default(),
            }),
            prompt_changed: Condvar::new(),
            quitting: AtomicBool::new(false),
            display: DisplayPort::new(),
        });
        shared.state.lock().unwrap_or_else(|e|e.into_inner()).download_hud=super::download_hud::DownloadHud::new(theme.clone());
        let driver = Arc::new(Self { shared: shared.clone(), worker: Mutex::new(None), theme,
        });
        let weak = Arc::downgrade(&shared);
        let worker = thread::spawn(move || worker(weak));
        *driver.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(worker);
        Some(driver)
    }
    /// The REPL keeps its blocking boundary; this owner retains Run-time draft
    /// and bracketed paste, then returns submitted text for normal admission.
    pub fn read_prompt(&self) -> Result<Option<String>, String> {
        self.read_prompt_inner(None)
    }
    pub fn read_prompt_with_ticket(&self, ticket: &Arc<PromptReadTicket>,
    ) -> Result<Option<String>, String> {
        self.read_prompt_inner(Some(ticket))
    }
    fn read_prompt_inner(&self, ticket: Option<&Arc<PromptReadTicket>>,
    ) -> Result<Option<String>, String> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        // Cancellation can precede this blocking closure being scheduled.
        // Consume the ticket before queued input or terminal ownership.
        if state.prompt_cancelled {
            state.prompt_cancelled = false;
            return Err("terminal prompt interrupted".into());
        }
        if state.raw {
            return Err("terminal already owns input".into());
        }
        if let Some(input) = state.input.submitted.pop_front() {
            return Ok(Some(input));
        }
        if ticket.is_some_and(|ticket| ticket.suspended.load(Ordering::Acquire)) {
            return Err(PASSIVE_PROMPT_SUSPENDED.into());
        }
        state.prompt_ticket = ticket.map(Arc::downgrade);
        state.prompt = true;
        state.input.result = None;
        state.write_failed = false;
        crossterm::terminal::enable_raw_mode().map_err(|error| error.to_string())?;
        state.raw = true;
        if let Err(error) = state.native_input.start() {
            let _ = restore(&mut state);
            return Err(error.to_string());
        }
        if let Err(error) = safe_write("\x1b[?25l\x1b[?2004h") {
            let _ = restore(&mut state);
            state.prompt = false;
            state.prompt_ticket = None;
            return Err(error.to_string());
        }
        *ACTIVE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::downgrade(&self.shared));
        if let Err(error) = paint(&mut state) {
            let _ = restore(&mut state);
            state.prompt = false;
            state.prompt_ticket = None;
            return Err(error.to_string());
        }
        while state.input.result.is_none() && !self.shared.quitting.load(Ordering::Acquire) {
            state = self.shared.prompt_changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        let result = state.input.result.take().unwrap_or_else(|| Err("terminal prompt interrupted".into()));
        state.prompt_cancelled = false;
        let restored = restore(&mut state);
        state.prompt = false;
        state.prompt_ticket = None;
        *ACTIVE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()) = None;
        match restored {
            Ok(()) => result,
            Err(error) => Err(error.to_string()),
        }
    }
    /// Wake and join one specific passive reader without overwriting a
    /// completed user line. An unscheduled ticket is observed by that reader
    /// alone; no global cancel ticket leaks into the next prompt.
    pub fn suspend_prompt(&self, ticket: &Arc<PromptReadTicket>) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        passive_suspend(&mut state, ticket);
        self.shared.prompt_changed.notify_all();
    }
    /// Cancel the pending prompt task, including one not yet scheduled.
    /// The Host joins that task before leaving the REPL; this does not cancel a Run.
    pub fn cancel_prompt(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.prompt_cancelled = true;
        if state.prompt {
            state.input.result = Some(Err("terminal prompt interrupted".into()));
        }
        self.shared.prompt_changed.notify_all();
    }
    pub fn begin_run(&self, mode: ara_edit::EditMode, cancel: CancellationToken,
    ) -> io::Result<u64> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.raw {
            return Err(io::Error::other("terminal already owns an active Run"));
        }
        crossterm::terminal::enable_raw_mode()?;
        state.raw = true;
        if let Err(error) = state.native_input.start() {
            let _ = restore(&mut state);
            return Err(error.into());
        }
        state.write_failed = false;
        let _ = mode;
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.active_generation = Some(generation);
        self.shared.display.run_generation(generation);
        state.cancel = Some(cancel);
        state.dirty = true;
        if let Err(error) = safe_write("\x1b[?25l\x1b[?2004h") {
            let _ = restore(&mut state);
            state.active_generation = None;
            state.cancel = None;
            return Err(error);
        }
        *ACTIVE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::downgrade(&self.shared));
        Ok(generation)
    }
    pub fn display_port(&self) -> Arc<DisplayPort> {
        self.shared.display.clone()
    }
    pub fn theme(&self) -> Arc<Theme> {
        self.theme.clone()
    }
    /// A capability change invalidates the same locked snapshot that gates
    /// publication. Hyperlinks retain their existing independent frame owner.
    pub fn set_markdown_capabilities(&self, capabilities: MarkdownCapabilities) -> bool {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.frame.true_color == capabilities.true_color
                && state.frame.text_sizing == capabilities.text_sizing
                && state.frame.image_protocol == capabilities.image_protocol
            {
                return false;
            }
            state.frame.true_color = capabilities.true_color;
            state.frame.text_sizing = capabilities.text_sizing;
            state.frame.image_protocol = capabilities.image_protocol;
            state.dirty = true;
        }
        // Queue the existing evaluator's frame demand after releasing the
        // output lock. No second writer or capability-specific scheduler.
        self.shared.display.frame();
        true
    }
    pub(crate) fn display_snapshot(&self) -> DisplaySnapshot {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        DisplaySnapshot {
            frame: s.frame,
            spinner: s.spinner,
            presentation_now: Instant::now(),
            generation: s.generation,
            prompt: s.prompt,
            native: s.native.active(),
        }
    }
    pub(crate) fn snapshot_current(&self, snapshot: &DisplaySnapshot) -> bool {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        snapshot.matches(&s)
    }
    pub(crate) fn take_history_ack(&self) -> Option<u64> {
        self.take_history_receipt().map(|ack| ack.id)
    }
    pub(crate) fn take_history_receipt(&self) -> Option<HistoryWriteReceipt> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).history_ack.take()
    }
    pub(crate) fn publish_owned(&self, snapshot: DisplaySnapshot, plan: FramePlan,
    ) -> io::Result<()> {
        self.publish_transaction(snapshot, plan, false, || {}).map(|_| ())
    }
    pub(crate) fn publish_native_transaction(
        &self,
        snapshot: DisplaySnapshot,
        surface: super::tsp_backend::Surface,
        commit: impl FnOnce(),
    ) -> io::Result<bool> {
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !snapshot.matches(&s) {
            return Ok(false);
        }
        commit();
        s.native_surface = surface;
        s.dirty = true;
        if s.raw && !s.write_failed {
            paint(&mut s)?;
        }
        Ok(true)
    }
    pub(crate) fn publish_transaction(
        &self,
        snapshot: DisplaySnapshot,
        plan: FramePlan,
        spinner_active: bool,
        commit: impl FnOnce(),
    ) -> io::Result<bool> {
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !snapshot.matches(&s) {
            return Ok(false);
        }
        commit();
        s.spinner_active = spinner_active;
        s.owned = plan;
        s.dirty = true;
        if s.raw && !s.write_failed {
            paint(&mut s)?;
        }
        Ok(true)
    }
    pub fn end_run(&self, generation: u64) -> io::Result<()> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.active_generation != Some(generation) {
            return Ok(());
        }
        self.shared.display.run_generation(0);
        // Semantic finish ran on the main-task evaluator before Run revoke.
        let paint_result = paint(&mut state);
        let restore_result = restore(&mut state);
        state.active_generation = None;
        state.cancel = None;
        *ACTIVE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner()) = None;
        paint_result.and(restore_result)
    }
    pub fn diagnostic(&self, text: &str) {
        report_on(&self.shared, text);
    }
}
impl Drop for TerminalDriver {
    fn drop(&mut self) {
        self.shared.quitting.store(true, Ordering::Release);
        self.shared.prompt_changed.notify_all();
        self.shared.display.close();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let _ = restore(&mut state);
    }
}
/// StreamingEditSink uses this synchronous output entry and keeps its original
/// cancellation/liveness lock. It never spawns a second terminal writer.
pub fn report_diagnostic(text: &str) {
    let active = ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(active) = active {
        report_on(&active, text);
    } else {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{text}");
        let _ = stderr.flush();
    }
}
fn report_on(shared: &Shared, text: &str) {
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.raw {
        let result =
            clear_viewport(&state.painted).and_then(|_| safe_write(&format!("{}\r\n", text.replace('\n', "\r\n"))));
        if let Err(error) = result {
            state.write_failed = true;
            state.input.result = Some(Err(error.to_string()));
            if let Some(cancel) = &state.cancel {
                cancel.cancel();
            }
            let _ = restore(&mut state);
            shared.prompt_changed.notify_all();
            return;
        }
        state.painted.clear();
        state.dirty = true;
        if let Err(error) = paint(&mut state) {
            state.write_failed = true;
            state.input.result = Some(Err(error.to_string()));
            if let Some(cancel) = &state.cancel {
                cancel.cancel();
            }
            let _ = restore(&mut state);
            shared.prompt_changed.notify_all();
        }
    } else {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{text}");
    }
}
fn worker(weak: Weak<Shared>) {
    let mut tick = Instant::now();
    loop {
        let Some(shared) = weak.upgrade() else {
            break;
        };
        if shared.quitting.load(Ordering::Acquire) {
            break;
        }
        // Capacity is reserved outside terminal/output locks. A saturated
        // lane pauses consumption; no cooked input can slip past raw ownership.
        let reservation = match shared.display.reserve() {
            Ok(slot) => slot,
            Err(super::evaluation::Admission::Closed) => break,
            Err(_) => {
                drop(shared);
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        let mut control = None;
        let mut reservation = Some(reservation);
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        {let State{download_events,download_hud,dirty,..}=&mut *state;
            *dirty|=download_hud.pump(download_events,Instant::now());}
        if !state.raw {
            drop(state);
            drop(shared);
            thread::sleep(Duration::from_millis(40));
            continue;
        }
        if state.prompt && state.input.result.is_some() {
            shared.prompt_changed.notify_all();
            drop(state);
            drop(shared);
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        // The same raw owner frames APC/OSC replies before key/paste decoding.
        // There is no concurrent crossterm reader that can discard reports.
        let native_before = state.native.active();
        let event = match state.native_input.poll(Duration::from_millis(20)) {
            Ok(Some(super::tsp_wire::Packet::Event(event))) => Ok(Some(event)),
            Ok(Some(super::tsp_wire::Packet::Da1)) => {
                state.native.da1();
                state.dirty = true;
                Ok(None)
            }
            Ok(Some(super::tsp_wire::Packet::Tsp(message))) => {
                if let Some(dispatch) = state.native.message(message) {
                    match dispatch {
                        super::tsp_backend::Dispatch::Resize(columns) => {
                            if state.frame.columns != columns {
                                state.frame.columns = columns;
                                state.frame.width_epoch = state.frame.width_epoch.wrapping_add(1);
                            }
                            control = Some(Control::Replay);
                        }
                        super::tsp_backend::Dispatch::Theme(_)
                        | super::tsp_backend::Dispatch::Motion(_) => {
                            control = Some(Control::Invalidate)
                        }
                        super::tsp_backend::Dispatch::Toggle(
                            super::tsp_backend::Action::Tool { generation, id },
                            expanded,
                        ) => {
                            control = Some(Control::ToolExpanded {
                                generation,
                                id,
                                expanded,
                            })
                        }
                        super::tsp_backend::Dispatch::Toggle(_, _) => {}
                        super::tsp_backend::Dispatch::Edit {
                            from,
                            to,
                            text,
                            cursor,
                            len,
                        } => state.input.native_edit(from, to, &text, cursor, len),
                        super::tsp_backend::Dispatch::Undo => state.input.native_undo(),
                        super::tsp_backend::Dispatch::Send(text) => {
                            let prompt = state.prompt;
                            state.input.native_send(&text, prompt)
                        }
                    }
                }
                state.dirty = true;
                Ok(None)
            }
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        if native_before != state.native.active() {
            control = Some(Control::Invalidate);
            state.dirty = true;
        }
        if state.native.needs_tick(Instant::now()) {
            state.dirty = true;
        }
        match event {
            Ok(Some(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
                    control = Some(Control::Expand);
                    state.dirty = true;
                } else if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                    && !state.prompt
                {
                    if let Some(cancel) = &state.cancel {
                        cancel.cancel();
                    }
                } else {
                    let prompt = state.prompt;
                    state.input.key(key, prompt);
                    state.dirty = true;
                }
            }
            Ok(Some(Event::Paste(text))) => {
                state.input.paste(&text);
                state.dirty = true;
            }
            Ok(Some(Event::Resize(columns, rows))) => {
                let width_changed = state.frame.columns != columns as usize;
                state.frame.columns = columns as usize;
                state.frame.rows = rows as usize;
                if width_changed {
                    state.frame.width_epoch = state.frame.width_epoch.wrapping_add(1);
                    control = Some(Control::Replay);
                }
                state.dirty = true;
            }
            Err(error) => {
                state.input.result = Some(Err(error.to_string()));
                if let Some(cancel) = &state.cancel {
                    cancel.cancel();
                }
                let _ = restore(&mut state);
                shared.prompt_changed.notify_all();
            }
            _ => {}
        }
        if state.prompt && state.input.result.is_some() {
            shared.prompt_changed.notify_all();
            continue;
        }
        if !state.prompt && state.spinner_active && tick.elapsed() >= Duration::from_millis(80) {
            state.spinner = state.spinner.wrapping_add(1);
            state.dirty = true;
            tick = Instant::now();
        }
        let demand = state.dirty && !state.prompt;
        if state.dirty && !state.write_failed {
            if let Err(error) = paint(&mut state) {
                state.write_failed = true;
                state.input.result = Some(Err(error.to_string()));
                if let Some(cancel) = &state.cancel {
                    cancel.cancel();
                }
                let _ = restore(&mut state);
                shared.prompt_changed.notify_all();
            }
        }
        drop(state);
        if let Some(control) = control {
            let _ = shared.display.control_reserved(reservation.take().unwrap(), control);
        }
        if demand {
            shared.display.frame();
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct DisplaySnapshot {
    pub frame: FrameContext,
    pub spinner: usize,
    pub presentation_now: Instant,
    generation: u64,
    prompt: bool,
    pub native: bool,
}
impl DisplaySnapshot {
    fn matches(&self, state: &State) -> bool {
        self.frame == state.frame
            && self.spinner == state.spinner
            && self.generation == state.generation
            && self.prompt == state.prompt
            && self.native == state.native.active()
    }
}
fn paint(state: &mut State) -> io::Result<()> {
    if state.write_failed {
        return Ok(());
    }
    let frame = state.frame;
    let spinner = state.spinner;
    let _ = spinner;
    let download_now=Instant::now();
    let download_rows=state.download_hud.render(frame.columns,download_now);
    let mut plan = if state.prompt {
        FramePlan {
            history: state.owned.history.clone(),
            viewport: {
                let mut rows = state.owned.viewport.clone();
                rows.extend(state.input.rows(frame.columns.max(1), frame.rows.max(1)));
                rows
            },
        }
    } else {
        state.owned.clone()
    };
    if !state.prompt && state.active_generation.is_some() && !state.input.draft.is_empty() {
        plan.viewport.extend(state.input.rows(frame.columns.max(1), 1));
    }
    let mut surface = state.native_surface.clone();
    if surface.main.is_empty() {
        surface.main.push(super::tsp_backend::Node::rows(
            "ansi-retained",
            plan.history
                .iter()
                .flat_map(|h| h.rows.clone())
                .chain(plan.viewport.clone())
                .collect(),
            frame.columns,
        ));
    }
    // The native main fallback was built before adding ANSI HUD rows, so
    // downloads are not duplicated between retained rows and the native dock.
    let download_at=state.owned.viewport.len().min(plan.viewport.len());
    plan.viewport.splice(download_at..download_at,download_rows.clone());
    if !download_rows.is_empty() {surface.dock.push(state.download_hud.describe(download_now));}
    if state.prompt || state.active_generation.is_some() {
        let mut editor = super::tsp_backend::Node::new(
            "composer",
            "editor",
            serde_json::json!({"text":state.input.draft,"cursor":state.input.draft[..state.input.cursor].encode_utf16().count(),"placeholder":"Message ARA","role":"omp.prompt.editor","sendable":true}),
            vec![],
        );
        editor.action = Some(super::tsp_backend::Action::Composer);
        surface.dock.push(editor);
    }
    let native = state.native.paint(surface, Instant::now());
    let wire = state.native.take_output();
    let output=native_frame_output(&state.painted,&native,&wire);
    if !output.is_empty() {
        if let Err(error)=safe_write(&output){state.write_failed=true;return Err(error);}
    }
    if matches!(native,super::tsp_backend::Paint::Native) {state.painted.clear();}
    match native {
        super::tsp_backend::Paint::Native => {
            state.dirty = false;
            return Ok(());
        }
        super::tsp_backend::Paint::Hold => return Ok(()),
        super::tsp_backend::Paint::Rows => {}
    }
    let bytes = compose_frame(&state.painted, &plan);
    if bytes.is_empty() {
        state.dirty = false;
        return Ok(());
    }
    match safe_write(&bytes) {
        Ok(()) => {
            if let Some(history) = plan.history {
                state.history_ack = Some(HistoryWriteReceipt { id: history.id, kind: history.kind,
                });
                state.owned.history = None;
            }
            state.painted = plan.viewport;
            state.dirty = false;
            Ok(())
        }
        Err(error) => {
            state.write_failed = true;
            Err(error)
        }
    }
}
// Erase only the last ANSI viewport, before opening the native surface. Native
// ACK/repaint has no row cache and emits no erasure; scrollback is preserved.
pub(super) fn native_frame_output(previous:&[String],paint:&super::tsp_backend::Paint,wire:&str)->String {
    let mut out=String::new();
    if matches!(paint,super::tsp_backend::Paint::Native)&&!previous.is_empty() {
        out.push('\r');if previous.len()>1 {out.push_str(&format!("\x1b[{}A",previous.len()-1));}out.push_str("\x1b[J");
    }
    out.push_str(wire);out
}
fn clear_viewport(previous: &[String]) -> io::Result<()> {
    if previous.is_empty() {
        return Ok(());
    }
    let mut bytes = "\r".to_owned();
    if previous.len() > 1 {
        bytes.push_str(&format!("\x1b[{}A", previous.len() - 1));
    }
    bytes.push_str("\x1b[J");
    safe_write(&bytes)
}
pub fn compose_frame(previous: &[String], plan: &FramePlan) -> String {
    if plan.history.is_none() && previous == plan.viewport {
        return String::new();
    }
    let mut out = "\x1b[?25l\x1b[?2026h\x1b[?7l\r".to_owned();
    if plan.history.as_ref().is_some_and(|h| h.kind == HistoryKind::ReplaceCollapsed) {
        // Fixed compaction replacement clears native scrollback only here.
        // Its receipt is produced by paint after safe_write, never by commit.
        out.push_str("\x1b[3J\x1b[2J\x1b[H");
    } else if !previous.is_empty() && previous.len() > 1 {
        out.push_str(&format!("\x1b[{}A", previous.len() - 1));
    }
    out.push_str("\x1b[J");
    if let Some(history) = &plan.history {
        if history.kind == HistoryKind::Replay {
            // append resize mode: the full logical ledger is re-offered below
            // retained native history, without retracting Task/Session state.
        }
        for row in &history.rows {
            out.push_str(row);
            out.push_str("\x1b[0m\x1b]8;;\x07\r\n");
        }
    }
    for (i, row) in plan.viewport.iter().enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        out.push_str(row);
        out.push_str("\x1b[0m\x1b]8;;\x07");
    }
    out.push_str("\x1b[?7h\x1b[?2026l");
    out
}
/// Fixed ConPTY 16KiB byte cap, preferring newline and retaining UTF8 boundaries.
pub fn conpty_chunks(data: &str, maximum: usize) -> Vec<&str> {
    if data.len() <= maximum {
        return vec![data];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let mut end = (start + maximum.max(4)).min(data.len());
        while !data.is_char_boundary(end) {
            end -= 1;
        }
        if end < data.len() {
            if let Some(newline) = data[start..end].rfind('\n') {
                end = start + newline + 1;
            }
        }
        out.push(&data[start..end]);
        start = end;
    }
    out
}
fn passive_suspend(state: &mut State, ticket: &Arc<PromptReadTicket>) {
    ticket.suspended.store(true, Ordering::Release);
    if state.prompt
        && state.prompt_ticket.as_ref().and_then(Weak::upgrade).is_some_and(|active| Arc::ptr_eq(&active, ticket))
        && state.input.result.is_none()
    {
        state.input.result = Some(Err(PASSIVE_PROMPT_SUSPENDED.into()));
    }
}
fn safe_write(bytes: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    if cfg!(windows) {
        for chunk in conpty_chunks(bytes, 16 * 1024) {
            stdout.write_all(chunk.as_bytes())?;
        }
    } else {
        stdout.write_all(bytes.as_bytes())?;
    }
    stdout.flush()
}
fn restore(state: &mut State) -> io::Result<()> {
    if !state.raw {
        return Ok(());
    }
    state.raw = false;
    state.native.stop(true);
    let wire = state.native.take_output();
    let native_write = if wire.is_empty() {
        Ok(())
    } else {
        safe_write(&wire)
    };
    let input_restore = state.native_input.stop();
    let write = safe_write("\x1b[0m\x1b]8;;\x07\x1b[?7h\x1b[?2026l\x1b[?2004l\x1b[?25h\r\n");
    let raw = crossterm::terminal::disable_raw_mode();
    state.painted.clear();
    native_write.and(input_restore).and(write).and(raw)
}

// Existing 31-observation renderer/terminal coarse family; no new test entrance.
#[cfg(test)]
pub(super) fn observe_passive_prompt_races() {
    let driver = TerminalDriver {
        shared: Arc::new(Shared {
            state: Mutex::new(State {
                owned: FramePlan { history: None, viewport: Vec::new(),
                },
                generation: 0,
                history_ack: None,
                frame: FrameContext { columns: 80, rows: 24, ..FrameContext::default() },
                painted: Vec::new(),
                spinner: 0,
                spinner_active: false,
                dirty: false,
                raw: false,
                active_generation: None,
                cancel: None,
                write_failed: false,
                prompt: false,
                prompt_cancelled: false,
                prompt_ticket: None,
                input: InputBuffer::default(),
                download_events: crate::download_activity::subscribe(),
                download_hud: Default::default(),
                native_input: Default::default(),
                native: Default::default(),
                native_surface: Default::default(),
            }),
            prompt_changed: Condvar::new(),
            quitting: AtomicBool::new(false),
            display: DisplayPort::new(),
        }),
        worker: Mutex::new(None),
        theme: Arc::new(Theme::dark()),
    };
    let ticket = Arc::new(PromptReadTicket::default());
    {
        let mut state = driver.shared.state.lock().unwrap();
        state.prompt = true;
        state.prompt_ticket = Some(Arc::downgrade(&ticket));
        state.input.insert("accepted Enter");
        state.input.key(event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), true,
        );
    }
    driver.suspend_prompt(&ticket);
    {
        let mut state = driver.shared.state.lock().unwrap();
        assert_eq!(state.input.result.take().unwrap(), Ok(Some("accepted Enter\n".into())));
        assert!(!state.prompt_cancelled);
        state.prompt = false;
        state.prompt_ticket = None;
    }
    let next = Arc::new(PromptReadTicket::default());
    assert!(!next.suspended.load(Ordering::Acquire));
    // A completed EOF owns the exit observation and cannot become a passive
    // suspension error (or the next prompt's explicit Ctrl+C cancel ticket).
    {
        let mut state = driver.shared.state.lock().unwrap();
        state.prompt = true;
        state.prompt_ticket = Some(Arc::downgrade(&next));
        state.input.result = Some(Ok(None));
    }
    driver.suspend_prompt(&next);
    {
        let mut state = driver.shared.state.lock().unwrap();
        assert_eq!(state.input.result.take().unwrap(), Ok(None));
        assert!(!state.prompt_cancelled);
        state.prompt = false;
        state.prompt_ticket = None;
    }
    // Suspension before closure scheduling is scoped to that exact ticket.
    // Already submitted input is preserved by the real reader preflight.
    let early = Arc::new(PromptReadTicket::default());
    driver.suspend_prompt(&early);
    assert_eq!(driver.read_prompt_with_ticket(&early), Err(PASSIVE_PROMPT_SUSPENDED.into()));
    driver.shared.state.lock().unwrap().input.submitted.push_back("queued Enter\n".into());
    assert_eq!(driver.read_prompt_with_ticket(&early), Ok(Some("queued Enter\n".into())));
    let active = Arc::new(PromptReadTicket::default());
    let unrelated = Arc::new(PromptReadTicket::default());
    {
        let mut state = driver.shared.state.lock().unwrap();
        state.prompt = true;
        state.prompt_ticket = Some(Arc::downgrade(&active));
        state.input.insert("retained draft");
    }
    driver.suspend_prompt(&unrelated);
    assert!(driver.shared.state.lock().unwrap().input.result.is_none());
    driver.suspend_prompt(&active);
    {
        let mut state = driver.shared.state.lock().unwrap();
        assert_eq!(state.input.result.take().unwrap(), Err(PASSIVE_PROMPT_SUSPENDED.into()));
        assert_eq!(state.input.draft, "retained draft");
        assert!(!state.prompt_cancelled);
        state.prompt = false;
        state.prompt_ticket = None;
    }
    assert!(!Arc::new(PromptReadTicket::default()).suspended.load(Ordering::Acquire));
    // Extend this existing coarse owner test with actual snapshot/publish
    // admission. Capability-only changes must reject an obsolete frame and
    // its commit callback, without touching the effective hyperlink owner.
    let stale = driver.display_snapshot();
    let capabilities = MarkdownCapabilities {
        true_color: true,
        text_sizing: true,
        image_protocol: Some(super::types::TerminalImageProtocol::Kitty),
    };
    assert!(driver.set_markdown_capabilities(capabilities));
    assert!(!driver.snapshot_current(&stale));
    let mut committed = false;
    assert!(
        !driver
            .publish_transaction(
                stale,
                FramePlan { history: None, viewport: vec!["obsolete capabilities".into()] },
                false,
                || committed = true,
            )
            .unwrap()
    );
    assert!(!committed);
    assert!(driver.shared.state.lock().unwrap().owned.viewport.is_empty());
    let current = driver.display_snapshot();
    assert_eq!(current.frame.hyperlinks, stale.frame.hyperlinks);
    assert_eq!(current.frame.true_color, capabilities.true_color);
    assert_eq!(current.frame.text_sizing, capabilities.text_sizing);
    assert_eq!(current.frame.image_protocol, capabilities.image_protocol);
    assert!(!driver.set_markdown_capabilities(capabilities));
    assert!(driver.snapshot_current(&current));
    assert!(
        driver
            .publish_transaction(
                current,
                FramePlan { history: None, viewport: vec!["current capabilities".into()] },
                false,
                || committed = true,
            )
            .unwrap()
    );
    assert!(committed);
    assert_eq!(driver.shared.state.lock().unwrap().owned.viewport, vec!["current capabilities"]);
    super::evaluation::observe_summary_capability_transaction(Arc::new(driver));
}

// Test-only original writer fixture: production paint and safe_write remain sole owners.
#[cfg(test)]
pub(super) fn emit_original_writer_rows(rows: Vec<String>) -> io::Result<()> {
    assert!(io::stdout().is_terminal(), "writer oracle requires an actual terminal");
    assert_eq!(crossterm::terminal::size()?, (80, 6));
    let mut state = State {
        owned: FramePlan { history: None, viewport: rows,
        },
        generation: 0,
        history_ack: None,
        frame: FrameContext { columns: 80, rows: 6, ..FrameContext::default() },
        painted: Vec::new(),
        spinner: 0,
        spinner_active: false,
        dirty: true,
        raw: false,
        active_generation: None,
        cancel: None,
        write_failed: false,
        prompt: false,
        prompt_cancelled: false,
        prompt_ticket: None,
        input: InputBuffer::default(),
                download_events: crate::download_activity::subscribe(),
                download_hud: Default::default(),
        native_input: Default::default(),
        native: Default::default(),
        native_surface: Default::default(),
    };
    // Reset the libtest heading's screen/cursor/styles before the actual frame.
    // The ConPTY oracle keeps all raw bytes, including the earlier heading.
    safe_write("\x1b[0m\x1b[3J\x1b[2J\x1b[H")?;
    paint(&mut state)?;
    assert!(!state.write_failed && !state.dirty);
    assert_eq!(state.painted, state.owned.viewport);
    Ok(())
}
