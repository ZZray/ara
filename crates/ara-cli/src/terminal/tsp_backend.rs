//! Native Tern surfaces, stable reconciliation and credit-controlled frames.
//! OMP b91db66e tui/src/native/{backend,reconcile,apply}.ts; MIT notices.
use super::tsp_wire::{self, Message};
use ara_rpc::WireString;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum Action {
    Tool { generation: u64, id: WireString },
    Composer,
}
#[derive(Clone, Debug)]
pub struct Node {
    pub key: String,
    pub kind: String,
    pub props: Value,
    pub children: Vec<Node>,
    pub action: Option<Action>,
}
impl Node {
    pub fn new(key: impl Into<String>, kind: &str, props: Value, children: Vec<Node>) -> Self {
        Self {
            key: key.into(),
            kind: kind.into(),
            props,
            children,
            action: None,
        }
    }
    pub fn rows(key: impl Into<String>, lines: Vec<String>, cols: usize) -> Self {
        Self::new(key, "rows", json!({"lines":lines,"cols":cols}), vec![])
    }
}
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub main: Vec<Node>,
    pub dock: Vec<Node>,
    pub layer: Vec<Node>,
}
#[derive(Clone, Debug)]
pub enum Dispatch {
    Resize(usize),
    Theme(bool),
    Motion(bool),
    Toggle(Action, bool),
    Edit {
        from: usize,
        to: usize,
        text: String,
        cursor: usize,
        len: usize,
    },
    Undo,
    Send(String),
}
#[derive(Clone, Debug)]
struct Hello {
    kinds: HashSet<String>,
    features: HashSet<String>,
    apc: usize,
    credits: usize,
}
impl Hello {
    fn read(value: &Value) -> Option<Self> {
        if value["v"].as_u64() != Some(tsp_wire::VERSION) {
            return None;
        }
        let strings = |key: &str| {
            value[key]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        Some(Self {
            kinds: strings("kinds"),
            features: strings("features"),
            apc: value["apc"].as_u64().unwrap_or(tsp_wire::DEFAULT_LIMIT as u64).max(1) as usize,
            credits: value["credits"]
                .as_u64()
                .unwrap_or(tsp_wire::DEFAULT_CREDITS as u64)
                .max(1) as usize,
        })
    }
    fn assumed() -> Self {
        Self{kinds:"col row card section rule spacer text md code diff ansi math image kv table tree badge kbd icon spinner shimmer elapsed progress rate list item tabs editor input status seg overlay toast rows picker prefs tool checklist agent chart meter effort".split_whitespace().map(str::to_owned).collect(),features:HashSet::from(["blobs".into()]),apc:tsp_wire::DEFAULT_LIMIT,credits:tsp_wire::DEFAULT_CREDITS}
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Sent {
    kind: String,
    props: Value,
    parent: String,
    children: Vec<String>,
}
pub enum Paint {
    Native,
    Hold,
    Rows,
}
/// All bytes are returned to TerminalDriver's existing output owner. The
/// backend never reads stdin or writes stdout on its own.
pub struct Backend {
    id: String,
    started: bool,
    pending: bool,
    optimistic: bool,
    tmux: bool,
    columns: usize,
    started_at: Option<Instant>,
    hello: Option<Hello>,
    opened: bool,
    adopt: bool,
    seq: u64,
    acked: u64,
    unacked: VecDeque<(u64, Instant)>,
    chunk: u64,
    out: String,
    ids: HashMap<String, String>,
    next_id: u64,
    sent: HashMap<String, Sent>,
    actions: HashMap<String, Action>,
    focus: Option<String>,
    latest: Option<Surface>,
    dirty: bool,
    visible: bool,
    pub dark: bool,
    pub reduce_motion: bool,
}
impl Default for Backend {
    fn default() -> Self {
        Self {
            id: "ara-session".into(),
            started: false,
            pending: false,
            optimistic: false,
            tmux: false,
            columns: 80,
            started_at: None,
            hello: None,
            opened: false,
            adopt: false,
            seq: 0,
            acked: 0,
            unacked: VecDeque::new(),
            chunk: 0,
            out: String::new(),
            ids: HashMap::new(),
            next_id: 0,
            sent: HashMap::new(),
            actions: HashMap::new(),
            focus: None,
            latest: None,
            dirty: false,
            visible: true,
            dark: true,
            reduce_motion: false,
        }
    }
}
impl Backend {
    #[cfg(test)]
    pub(super) fn expect_hello_for_test(&mut self,now:Instant){self.started=true;self.pending=true;self.started_at=Some(now);}
    pub fn active(&self) -> bool {
        self.started && self.hello.is_some()
    }
    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.out)
    }
    fn send(&mut self, verb: char, value: Value) {
        let limit = self.hello.as_ref().map_or(tsp_wire::DEFAULT_LIMIT, |h| h.apc);
        self.out
            .push_str(&tsp_wire::encode(verb, &value, limit, &mut self.chunk));
    }
    pub fn begin(&mut self, now: Instant) {
        if self.started {
            return;
        }
        self.started = true;
        self.started_at = Some(now);
        let override_value = std::env::var("ARA_TUI_NATIVE").ok();
        self.tmux = std::env::var_os("TMUX").is_some();
        let mux = self.tmux || std::env::var_os("STY").is_some();
        let tmux_supported =
            self.tmux && run_tmux(&["show-options", "-qv", "@tern-tsp"]).is_some_and(|value| !value.trim().is_empty());
        if override_value.as_deref() == Some("0")
            || (self.tmux && !tmux_supported)
            || (!self.tmux && mux && override_value.as_deref() != Some("1"))
        {
            self.hello=None;self.adopt=false;self.sent.clear();
            return;
        }
        self.pending = true;
        self.send(
            'q',
            json!({"q":"hello","v":[1],"app":"ara","ver":env!("CARGO_PKG_VERSION"),"features":["edit","undo","send"]}),
        );
        if self.tmux {
            tmux_pane_titles(false);
        } else {
            self.out.push_str("\x1b[c");
        }
        self.optimistic = !mux && std::env::var("TERM_PROGRAM").is_ok_and(|term| term.eq_ignore_ascii_case("tern"));
        if self.optimistic {
            self.hello = Some(Hello::assumed());
            self.dirty = true;
        }
    }
    pub fn da1(&mut self) {
        if self.pending && !self.tmux {
            self.pending = false;
            self.revoke();
        }
    }
    pub fn revoke(&mut self) {
        if self.tmux {
            tmux_pane_titles(true);
        }
        if self.opened {
            self.send('x', json!({"id":self.id,"keep":false}));
        }
        self.opened = false;
        self.adopt=false;
        self.hello = None;
        self.pending = false;
        self.optimistic = false;
        self.sent.clear();
        self.actions.clear();self.focus=None;
        self.unacked.clear();
        self.dirty = true;
    }
    pub fn stop(&mut self, keep: bool) {
        if self.tmux {tmux_pane_titles(true);}
        if self.opened {
            self.flush_stalled(Instant::now() + Duration::from_secs(5));self.frame(Instant::now());
            self.send('x',json!({"id":self.id,"keep":keep}));
        }
        self.opened=false;self.started=false;self.pending=false;self.optimistic=false;self.adopt=keep;
        if keep {
            // The terminal keeps main on x/keep. Preserve its exact sent ids;
            // next o/adopt re-adds only live dock/layer and diffs retained main.
            let mut retained=HashSet::from(["main".into()]);
            loop{let before=retained.len();for(id,node)in &self.sent{if retained.contains(&node.parent){retained.insert(id.clone());}}if retained.len()==before{break}}
            retained.insert(self.id.clone());
            self.sent.retain(|id,_|retained.contains(id));
            if let Some(root)=self.sent.get_mut(&self.id){root.children=vec!["main".into()];}
        } else {self.hello=None;self.sent.clear();}
        self.actions.clear();self.focus=None;self.unacked.clear();self.acked=self.seq;
    }
    pub fn needs_tick(&self, now: Instant) -> bool {
        (self.pending
            && self
                .started_at
                .is_some_and(|at| now.duration_since(at) >= Duration::from_millis(1000)))
            || (self.dirty
                && self
                    .unacked
                    .front()
                    .is_some_and(|(_, at)| now.duration_since(*at) >= Duration::from_secs(5)))
    }
    pub fn message(&mut self, message: Message) -> Option<Dispatch> {
        let value = &message.value;
        if message.verb == 'r' && value["r"] == "hello" {
            if !self.pending {return None}
            let Some(hello) = Hello::read(value) else {
                self.revoke();
                return None;
            };
            self.pending = false;
            self.optimistic = false;
            let changed = self
                .hello
                .as_ref()
                .is_some_and(|old| old.kinds != hello.kinds || old.features != hello.features);
            if changed && self.opened {
                self.send('x', json!({"id":self.id,"keep":false}));
                self.opened = false;
                self.sent.clear();
                self.unacked.clear();
            }
            self.hello = Some(hello);
            self.dark = value["dark"].as_bool().unwrap_or(true);
            self.reduce_motion = value["reduceMotion"].as_bool().unwrap_or(false);
            self.dirty = true;
            if let Some(cols) = value["cols"].as_u64() {
                self.columns = cols.max(1) as usize;
                return Some(Dispatch::Resize(self.columns));
            }
            return None;
        }
        if message.verb != 'e' || !self.active() {
            return None;
        }
        if value.get("sf").is_some_and(|sf| sf != &Value::String(self.id.clone())) {
            return None;
        }
        match value["ev"].as_str()? {
            "ack" => {
                let seq = value["s"].as_u64()?;
                self.acked = self.acked.max(seq.min(self.seq));
                while self.unacked.front().is_some_and(|(s, _)| *s <= self.acked) {
                    self.unacked.pop_front();
                }
                None
            }
            "resize" => {
                self.columns = value["cols"].as_u64()?.max(1) as usize;
                Some(Dispatch::Resize(self.columns))
            }
            "theme" => {
                self.dark = value["dark"].as_bool()?;
                Some(Dispatch::Theme(self.dark))
            }
            "motion" => {
                self.reduce_motion = value["reduce"].as_bool()?;
                Some(Dispatch::Motion(self.reduce_motion))
            }
            "visible" => {
                self.visible = value["visible"].as_bool()?;
                if self.visible {
                    self.dirty = true;
                }
                None
            }
            "toggle" => Some(Dispatch::Toggle(
                self.actions.get(value["id"].as_str()?)?.clone(),
                !value["collapsed"].as_bool()?,
            )),
            "edit" | "undo" | "send" => {
                if !matches!(self.actions.get(value["id"].as_str()?), Some(Action::Composer)) {
                    return None;
                }
                match value["ev"].as_str()? {
                    "edit" => Some(Dispatch::Edit {
                        from: value["from"].as_u64()? as usize,
                        to: value["to"].as_u64()? as usize,
                        text: value["text"].as_str()?.into(),
                        cursor: value["cursor"].as_u64()? as usize,
                        len: value["len"].as_u64()? as usize,
                    }),
                    "undo" => Some(Dispatch::Undo),
                    _ => Some(Dispatch::Send(value["text"].as_str()?.into())),
                }
            }
            "gone" => {
                if value["ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == &Value::String(self.id.clone())))
                {
                    self.opened = false;
                    self.sent.clear();
                    self.unacked.clear();
                    self.dirty = true;
                }
                None
            }
            _ => None,
        }
    }
    pub fn paint(&mut self, surface: Surface, now: Instant) -> Paint {
        self.begin(now);
        if self.pending
            && (self.optimistic || self.tmux)
            && self
                .started_at
                .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(1))
        {
            self.revoke();
        }
        if !self.active() {
            return Paint::Rows;
        }
        self.latest = Some(surface);
        self.dirty = true;
        self.flush_stalled(now);
        self.frame(now);
        Paint::Native
    }
    fn flush_stalled(&mut self, now: Instant) {
        if self
            .unacked
            .front()
            .is_some_and(|(_, at)| now.duration_since(*at) >= Duration::from_secs(5))
        {
            self.unacked.clear();
            self.acked = self.seq;
        }
    }
    fn frame(&mut self, now: Instant) {
        let Some(hello) = self.hello.clone() else {
            return;
        };
        if !self.visible || !self.dirty || self.unacked.len() >= hello.credits {
            return;
        }
        let Some(surface) = self.latest.clone() else {
            return;
        };
        if !self.opened {
            self.send(
                'o',
                json!({"id":self.id,"mode":"inline","role":"omp.session","title":"ARA","adopt":self.adopt}),
            );
            self.send('t', palette(&self.id));
            self.opened = true;self.adopt=false;
        }
        let root = self.id.clone();
        let mut next = HashMap::new();
        let mut order = vec![];
        let mut actions = HashMap::new();
        let regions = [("main", surface.main), ("dock", surface.dock), ("layer", surface.layer)];
        next.insert(
            root.clone(),
            Sent {
                kind: "col".into(),
                props: json!({}),
                parent: String::new(),
                children: regions.iter().map(|(id, _)| (*id).into()).collect(),
            },
        );
        for (region, children) in regions {
            let child_ids = children
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    self.assign(&format!(
                        "{region}/{}",
                        if node.key.is_empty() {
                            index.to_string()
                        } else {
                            node.key.clone()
                        }
                    ))
                })
                .collect::<Vec<_>>();
            next.insert(
                region.into(),
                Sent {
                    kind: "col".into(),
                    props: json!({"role":region}),
                    parent: root.clone(),
                    children: child_ids.clone(),
                },
            );
            order.push(region.to_owned());
            for ((index, node), id) in children.into_iter().enumerate().zip(child_ids) {
                let path = format!(
                    "{region}/{}",
                    if node.key.is_empty() {
                        index.to_string()
                    } else {
                        node.key.clone()
                    }
                );
                self.flatten(node, id, region, &path, &hello, &mut next, &mut order, &mut actions);
            }
        }
        let mut ops = Vec::new();
        let removed: HashSet<String> = self
            .sent
            .keys()
            .filter(|id| {
                *id != &root
                    && (!next.contains_key(*id) || next.get(*id).is_some_and(|n| self.sent[*id].kind != n.kind))
            })
            .cloned()
            .collect();
        let under_removed = |id: &str| {
            let mut current = id;
            while let Some(old) = self.sent.get(current) {
                if removed.contains(current) {
                    return true;
                }
                if old.parent.is_empty() {
                    break;
                }
                current = &old.parent;
            }
            false
        };
        for id in &removed {
            if !self.sent.get(id).is_some_and(|old| under_removed(&old.parent)) {
                ops.push(json!(["del", id]));
            }
        }
        for id in &order {
            let node = &next[id];
            match self.sent.get(id).filter(|_| !under_removed(id)) {
                Some(old) if old.kind == node.kind => {
                    if old.props != node.props {
                        let mut props = node.props.as_object().cloned().unwrap_or_default();
                        if let Some(previous) = old.props.as_object() {
                            for key in previous.keys() {
                                if !props.contains_key(key) {
                                    props.insert(key.clone(), Value::Null);
                                }
                            }
                        }
                        ops.push(json!(["set", id, props]));
                    }
                }
                old => {
                    if old.is_some() {
                        ops.push(json!(["del", id]));
                    }
                    ops.push(json!(["add",id,node.parent,Value::Null,{"id":id,"k":node.kind,"p":node.props}]));
                }
            }
        }
        for (parent, node) in &next {
            let previous = self.sent.get(parent).map(|sent| &sent.children);
            if previous != Some(&node.children) {
                let mut before: Option<&String> = None;
                for child in node.children.iter().rev() {
                    ops.push(json!(["move", child, parent, before]));
                    before = Some(child);
                }
            }
        }
        let focus=actions.iter().find_map(|(id,action)|matches!(action,Action::Composer).then(||id.clone()));
        if focus!=self.focus{ops.push(json!(["focus",focus]));self.focus=focus;}
        self.sent = next;
        self.actions = actions;
        self.dirty = false;
        if ops.is_empty() {
            return;
        }
        self.seq += 1;
        self.send('f', json!({"sf":self.id,"s":self.seq,"ops":ops}));
        self.unacked.push_back((self.seq, now));
    }
    fn assign(&mut self, key: &str) -> String {
        if let Some(id) = self.ids.get(key) {
            return id.clone();
        }
        self.next_id += 1;
        let id = format!("n{}", self.next_id);
        self.ids.insert(key.into(), id.clone());
        id
    }
    fn flatten(
        &mut self,
        mut node: Node,
        id: String,
        parent: &str,
        path: &str,
        hello: &Hello,
        next: &mut HashMap<String, Sent>,
        order: &mut Vec<String>,
        actions: &mut HashMap<String, Action>,
    ) {
        // JSON.stringify omits source undefined props. Missing properties are
        // encoded as null only in a later set op that removes an old value.
        if let Some(props)=node.props.as_object_mut(){props.retain(|_,value|!value.is_null());}
        if !hello.kinds.contains(&node.kind) {
            if node.kind == "tool" && hello.kinds.contains("card") {
                node.kind = "card".into();
                let title=node.props["title"].as_str().unwrap_or("");
                let target=node.props["target"].as_str().unwrap_or("");
                node.props["head"]=json!([{"t":title,"s":"toolTitle strong"},{"t":if target.is_empty(){String::new()}else{format!(" {target}")},"s":"muted"}]);
            } else if node.kind == "diff" && hello.kinds.contains("code") {
                if node.props.get("text").is_none(){let text=node.props["hunks"].as_array().into_iter().flatten().map(|h|format!("@@ -{} +{} @@\n{}",h["oldStart"],h["newStart"],h["lines"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))).collect::<Vec<_>>().join("\n");node.props["text"]=json!(text);}
                node.kind = "code".into();
            } else if node.children.is_empty() {
                let text = node
                    .props
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        node.props
                            .get("spans")
                            .or_else(|| node.props.get("head"))
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|s| s["t"].as_str())
                            .collect::<Vec<_>>()
                            .join("")
                    });
                node.kind = "rows".into();
                node.props = json!({"cols":self.columns,"lines":text.lines().flat_map(|line|super::render_utils::wrap(line,self.columns)).collect::<Vec<_>>()});
            } else {
                node.kind = "col".into();
                node.props = json!({});
            }
        }
        if let Some(action) = node.action {
            actions.insert(id.clone(), action);
        }
        let child_ids = node
            .children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                self.assign(&format!(
                    "{path}/{}",
                    if child.key.is_empty() {
                        index.to_string()
                    } else {
                        child.key.clone()
                    }
                ))
            })
            .collect::<Vec<_>>();
        next.insert(
            id.clone(),
            Sent {
                kind: node.kind,
                props: node.props,
                parent: parent.into(),
                children: child_ids.clone(),
            },
        );
        order.push(id.clone());
        for ((index, child), child_id) in node.children.into_iter().enumerate().zip(child_ids) {
            let child_path = format!(
                "{path}/{}",
                if child.key.is_empty() {
                    index.to_string()
                } else {
                    child.key.clone()
                }
            );
            self.flatten(child, child_id, &id, &child_path, hello, next, order, actions);
        }
    }
}

fn run_tmux(args: &[&str]) -> Option<String> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        thread,
    };
    let mut child = Command::new("tmux")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let until = Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut text = String::new();
                child.stdout.take()?.read_to_string(&mut text).ok()?;
                return Some(text);
            }
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
fn tmux_pane_titles(on: bool) {
    if let Ok(pane) = std::env::var("TMUX_PANE") {
        if on {
            let _ = run_tmux(&["set-option", "-pu", "-t", &pane, "allow-set-title"]);
        } else {
            let _ = run_tmux(&["set-option", "-p", "-t", &pane, "allow-set-title", "off"]);
        }
    }
}
fn palette(surface:&str)->Value{
    fn hex(value:&Value)->Option<String>{
        if let Some(value)=value.as_str().filter(|v|v.starts_with('#')&&v.len()==7){return Some(value.into())}
        let n=value.as_u64().filter(|n|*n<256)? as u32;
        let rgb=if n<16{[0x000000,0x800000,0x008000,0x808000,0x000080,0x800080,0x008080,0xc0c0c0,0x808080,0xff0000,0x00ff00,0xffff00,0x0000ff,0xff00ff,0x00ffff,0xffffff][n as usize]}
            else if n<232{let i=n-16;let c=|v:u32|if v==0{0}else{55+v*40};(c(i/36)<<16)|(c((i/6)%6)<<8)|c(i%6)}
            else{let c=8+(n-232)*10;(c<<16)|(c<<8)|c};Some(format!("#{rgb:06x}"))
    }
    fn variant(source:&str)->Value{let source:Value=serde_json::from_str(source).unwrap_or_default();let mut colors=serde_json::Map::new();for group in ["colors","export"]{if let Some(map)=source[group].as_object(){for(key,value)in map{let resolved=value.as_str().and_then(|name|source["vars"].get(name)).unwrap_or(value);if let Some(value)=hex(resolved){colors.insert(key.clone(),json!(value));}}}}Value::Object(colors)}
    let mut palette=json!({"sf":surface,"name":{}});
    let chosen=std::env::var("ARA_TERMINAL_THEME").ok();
    if chosen.as_deref()!=Some("light"){palette["dark"]=variant(include_str!("themes/dark.json"));palette["name"]["dark"]=json!("dark");}
    if chosen.as_deref()!=Some("dark"){palette["light"]=variant(include_str!("themes/light.json"));palette["name"]["light"]=json!("light");}
    palette
}
