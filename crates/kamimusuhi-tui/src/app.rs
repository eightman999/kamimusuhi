//! TUI state and event loop: mirrors the resident mode of the egui desktop
//! app (chat, task board, approval queue, node status, tool catalog).

use std::collections::HashSet;
use std::io;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use kamimusuhi_resident::remote::{RemoteCommand, RemoteEvent};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::Color;
use serde_json::Value;

use crate::ui;

/// The individual accepts at most this much per turn (runtime contract).
pub const MAX_INPUT_BYTES: usize = kamimusuhi_runtime::dialogue::MAX_INPUT_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Chat,
    Tasks,
    Approvals,
    Status,
    Tools,
}

impl View {
    pub const ALL: [Self; 5] = [
        Self::Chat,
        Self::Tasks,
        Self::Approvals,
        Self::Status,
        Self::Tools,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "対話",
            Self::Tasks => "タスク",
            Self::Approvals => "承認",
            Self::Status => "状態",
            Self::Tools => "ツール",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug)]
pub enum Entry {
    User { text: String, at: Option<String> },
    Assistant { text: String, meta: Option<Value> },
    Notice { text: String, color: Color },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    /// Composing a task title; `depends` links it after the selection.
    NewTask {
        depends: bool,
    },
    ToolFilter,
}

pub struct App {
    pub commands: Sender<RemoteCommand>,
    pub subject: String,
    pub url: Option<String>,
    pub view: View,
    pub entries: Vec<Entry>,
    pub input: String,
    /// Byte offset of the cursor inside `input`.
    pub cursor: usize,
    pub busy_since: Option<Instant>,
    pub error: Option<String>,
    pub status: Option<Value>,
    pub status_text: Option<String>,
    pub status_at: Option<Instant>,
    pub approvals: Vec<Value>,
    pub announced: HashSet<String>,
    pub deciding: HashSet<String>,
    pub last_decisions: Vec<(String, Value)>,
    pub approval_sel: usize,
    pub tasks: Vec<Value>,
    pub task_sel: usize,
    pub show_done: bool,
    pub tools: Option<Value>,
    pub tool_filter: String,
    pub tool_cursor: usize,
    pub mode: InputMode,
    pub chat_scroll: u16,
    pub chat_scroll_max: u16,
    pub chat_follow: bool,
    pub status_scroll: u16,
    pub tools_scroll: u16,
    pub tasks_scroll: u16,
    pub detail_scroll: u16,
    pub quit: bool,
}

/// Enter the alternate screen, run the loop, restore the terminal.
pub fn run(
    commands: Sender<RemoteCommand>,
    events: Receiver<RemoteEvent>,
    subject: String,
    initial_view: Option<String>,
) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste).map_err(|e| e.to_string())?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| e.to_string())?;
    let result = event_loop(&mut terminal, commands, events, subject, initial_view);
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    );
    let _ = terminal.show_cursor();
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    commands: Sender<RemoteCommand>,
    events: Receiver<RemoteEvent>,
    subject: String,
    initial_view: Option<String>,
) -> Result<(), String> {
    let mut app = App::new(commands, subject, initial_view);
    loop {
        while let Ok(event) = events.try_recv() {
            app.on_remote(event);
        }
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .map_err(|e| e.to_string())?;
        if app.quit {
            break;
        }
        if event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                Event::Paste(text) => app.insert_text(&text),
                _ => {}
            }
        }
    }
    let _ = app.commands.send(RemoteCommand::Shutdown);
    Ok(())
}

impl App {
    fn new(commands: Sender<RemoteCommand>, subject: String, initial_view: Option<String>) -> Self {
        let view = match initial_view.as_deref() {
            Some("tasks") => View::Tasks,
            Some("approvals") => View::Approvals,
            Some("status") => View::Status,
            Some("tools") => View::Tools,
            _ => View::Chat,
        };
        Self {
            commands,
            subject,
            url: None,
            view,
            entries: Vec::new(),
            input: String::new(),
            cursor: 0,
            busy_since: None,
            error: None,
            status: None,
            status_text: None,
            status_at: None,
            approvals: Vec::new(),
            announced: HashSet::new(),
            deciding: HashSet::new(),
            last_decisions: Vec::new(),
            approval_sel: 0,
            tasks: Vec::new(),
            task_sel: 0,
            show_done: true,
            tools: None,
            tool_filter: String::new(),
            tool_cursor: 0,
            mode: InputMode::Normal,
            chat_scroll: 0,
            chat_scroll_max: 0,
            chat_follow: true,
            status_scroll: 0,
            tools_scroll: 0,
            tasks_scroll: 0,
            detail_scroll: 0,
            quit: false,
        }
    }

    pub fn pending(&self) -> Vec<&Value> {
        self.approvals
            .iter()
            .filter(|a| a["state"] == "pending")
            .collect()
    }

    /// Tasks in board order: open lanes first, done last (newest first).
    pub fn visible_tasks(&self) -> Vec<&Value> {
        let mut out: Vec<&Value> = Vec::new();
        for lane in [
            "in_progress",
            "awaiting_operator",
            "waiting",
            "on_hold",
            "done",
        ] {
            if lane == "done" && !self.show_done {
                continue;
            }
            let mut lane_tasks: Vec<&Value> = self
                .tasks
                .iter()
                .filter(|t| {
                    let status = if t["status"] == "failed" {
                        "done"
                    } else {
                        t["status"].as_str().unwrap_or("")
                    };
                    status == lane
                })
                .collect();
            lane_tasks.sort_by_key(|t| t["created_at"].as_u64().unwrap_or(0));
            if lane == "done" {
                lane_tasks.reverse();
                lane_tasks.truncate(40);
            }
            out.extend(lane_tasks);
        }
        out
    }

    pub fn selected_task(&self) -> Option<&Value> {
        self.visible_tasks().get(self.task_sel).copied()
    }

    fn on_remote(&mut self, event: RemoteEvent) {
        match event {
            RemoteEvent::Connected { url, history } => {
                self.url = Some(url);
                self.error = None;
                for turn in history {
                    self.entries.push(Entry::User {
                        text: turn["message"].as_str().unwrap_or("").to_owned(),
                        at: turn["ts"].as_str().map(str::to_owned),
                    });
                    self.entries.push(Entry::Assistant {
                        text: turn["response"].as_str().unwrap_or("").to_owned(),
                        meta: Some(turn),
                    });
                }
                if !self.entries.is_empty() {
                    self.entries.push(Entry::Notice {
                        text: "── ここまでが保存済みの会話です ──".to_owned(),
                        color: ui::MUTED,
                    });
                }
            }
            RemoteEvent::Reply(reply) => {
                self.busy_since = None;
                self.error = None;
                self.entries.push(Entry::Assistant {
                    text: reply["response"].as_str().unwrap_or("").to_owned(),
                    meta: Some(reply),
                });
            }
            RemoteEvent::Status(status) => {
                self.status_text = Some(kamimusuhi_resident::status::render(&status).0);
                self.status = Some(status);
                self.status_at = Some(Instant::now());
            }
            RemoteEvent::Approvals(list) => {
                let first_load =
                    self.announced.is_empty() && self.url.is_some() && self.approvals.is_empty();
                for a in list.iter().filter(|a| a["state"] == "pending") {
                    let id = a["id"].as_str().unwrap_or("").to_owned();
                    if self.announced.insert(id) && !first_load {
                        self.entries.push(Entry::Notice {
                            text: format!("⚠ 承認待ちが追加されました: {}", ui::approval_title(a)),
                            color: ui::AMBER,
                        });
                    }
                }
                self.approvals = list;
            }
            RemoteEvent::Tools(tools) => self.tools = Some(tools),
            RemoteEvent::Tasks(tasks) => self.tasks = tasks,
            RemoteEvent::Decision { id, reply } => {
                self.deciding.remove(&id);
                let ok = reply["ok"].as_bool().unwrap_or(false);
                self.entries.push(Entry::Notice {
                    text: format!(
                        "{} 承認 {id}: {}",
                        if ok { "✔" } else { "✘" },
                        reply["state"]
                            .as_str()
                            .or_else(|| reply["approval"]["state"].as_str())
                            .unwrap_or("?")
                    ),
                    color: if ok { ui::GREEN } else { ui::RED },
                });
                self.last_decisions.insert(0, (id, reply));
                self.last_decisions.truncate(10);
            }
            RemoteEvent::Error(message) => {
                if message.starts_with("応答") {
                    self.busy_since = None;
                }
                self.error = Some(message);
            }
        }
    }

    // ── keys ────────────────────────────────────────────────────────────

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.mode = InputMode::Normal;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match key.code {
            KeyCode::F(n @ 1..=5) => return self.set_view(View::ALL[(n - 1) as usize]),
            KeyCode::Tab => return self.set_view(self.view.next()),
            KeyCode::BackTab => return self.set_view(self.view.prev()),
            _ => {}
        }
        match self.mode {
            InputMode::NewTask { depends } => self.new_task_key(key, depends),
            InputMode::ToolFilter => self.tool_filter_key(key),
            InputMode::Normal => match self.view {
                View::Chat => self.chat_key(key),
                View::Tasks => self.tasks_key(key),
                View::Approvals => self.approvals_key(key),
                View::Status => self.status_key(key),
                View::Tools => self.tools_key(key),
            },
        }
    }

    /// `text` goes to whichever input is active (paste path).
    pub fn insert_text(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| if c == '\r' { '\n' } else { c })
            .collect();
        match self.mode {
            InputMode::Normal if self.view == View::Chat => {
                insert_at(&mut self.input, &mut self.cursor, &text)
            }
            InputMode::NewTask { .. } => {
                insert_at(&mut self.input, &mut self.cursor, &text.replace('\n', " "))
            }
            InputMode::ToolFilter => insert_at(
                &mut self.tool_filter,
                &mut self.tool_cursor,
                &text.replace('\n', " "),
            ),
            _ => {}
        }
    }

    fn chat_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter if alt => insert_at(&mut self.input, &mut self.cursor, "\n"),
            KeyCode::Enter => self.send(),
            KeyCode::Char('n') if ctrl => insert_at(&mut self.input, &mut self.cursor, "\n"),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::PageUp => self.scroll_chat(-(self.chat_page() as i32)),
            KeyCode::PageDown => self.scroll_chat(self.chat_page() as i32),
            KeyCode::Up => self.scroll_chat(-1),
            KeyCode::Down => self.scroll_chat(1),
            _ => {
                edit_buffer(&mut self.input, &mut self.cursor, &key);
            }
        }
    }

    fn chat_page(&self) -> u16 {
        // Roughly one screen of scrollback; updated each draw.
        10
    }

    fn tasks_key(&mut self, key: KeyEvent) {
        let len = self.visible_tasks().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.task_sel = (self.task_sel + 1).min(len.saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => self.task_sel = self.task_sel.saturating_sub(1),
            KeyCode::Char('n') => {
                self.mode = InputMode::NewTask { depends: false };
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('N') => {
                self.mode = InputMode::NewTask { depends: true };
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('d') => self.mark_done(),
            KeyCode::Char('h') => self.show_done = !self.show_done,
            KeyCode::Char('r') => {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn approvals_key(&mut self, key: KeyEvent) {
        let len = self.pending().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.approval_sel = (self.approval_sel + 1).min(len.saturating_sub(1));
                self.detail_scroll = 0;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.approval_sel = self.approval_sel.saturating_sub(1);
                self.detail_scroll = 0;
            }
            KeyCode::Char('a') => self.decide_selected(true),
            KeyCode::Char('x') => self.decide_selected(false),
            KeyCode::Char('J') => self.detail_scroll = self.detail_scroll.saturating_add(3),
            KeyCode::Char('K') => self.detail_scroll = self.detail_scroll.saturating_sub(3),
            KeyCode::Char('r') => {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn status_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.status_scroll = self.status_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.status_scroll = self.status_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.status_scroll = self.status_scroll.saturating_add(10),
            KeyCode::PageUp => self.status_scroll = self.status_scroll.saturating_sub(10),
            KeyCode::Char('r') => {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn tools_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('/') => {
                self.mode = InputMode::ToolFilter;
                self.tool_cursor = self.tool_filter.len();
            }
            KeyCode::Char('c') => {
                self.tool_filter.clear();
                self.tool_cursor = 0;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.tools_scroll = self.tools_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.tools_scroll = self.tools_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.tools_scroll = self.tools_scroll.saturating_add(10),
            KeyCode::PageUp => self.tools_scroll = self.tools_scroll.saturating_sub(10),
            KeyCode::Char('r') => {
                let _ = self.commands.send(RemoteCommand::Refresh);
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn new_task_key(&mut self, key: KeyEvent, depends: bool) {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Enter => {
                let title = self.input.trim().to_owned();
                if !title.is_empty() {
                    let depends_on: Vec<String> = if depends {
                        self.selected_task()
                            .and_then(|t| t["id"].as_str().map(str::to_owned))
                            .into_iter()
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let _ = self.commands.send(RemoteCommand::Task(serde_json::json!({
                        "action": "create", "title": title,
                        "status": "waiting", "depends_on": depends_on})));
                }
                self.mode = InputMode::Normal;
                self.input.clear();
                self.cursor = 0;
            }
            _ => {
                edit_buffer(&mut self.input, &mut self.cursor, &key);
            }
        }
    }

    fn tool_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = InputMode::Normal,
            _ => {
                edit_buffer(&mut self.tool_filter, &mut self.tool_cursor, &key);
                self.tools_scroll = 0;
            }
        }
    }

    // ── actions ─────────────────────────────────────────────────────────

    fn send(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() || self.busy_since.is_some() || self.url.is_none() {
            return;
        }
        if text.len() > MAX_INPUT_BYTES {
            self.error = Some(format!("入力が {MAX_INPUT_BYTES} bytes を超えています。"));
            return;
        }
        self.entries.push(Entry::User {
            text: text.clone(),
            at: None,
        });
        self.input.clear();
        self.cursor = 0;
        self.error = None;
        self.chat_follow = true;
        self.busy_since = Some(Instant::now());
        if self.commands.send(RemoteCommand::Send { text }).is_err() {
            self.busy_since = None;
            self.error = Some("通信ワーカーが停止しています。".to_owned());
        }
    }

    fn decide_selected(&mut self, approve: bool) {
        let pending = self.pending();
        let Some(id) = pending
            .get(self.approval_sel)
            .and_then(|a| a["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        if self.deciding.insert(id.clone()) {
            let _ = self.commands.send(RemoteCommand::Decide {
                id: id.clone(),
                approve,
            });
        }
    }

    fn mark_done(&mut self) {
        let Some(id) = self
            .selected_task()
            .and_then(|t| t["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let _ = self.commands.send(RemoteCommand::Task(serde_json::json!({
            "action": "update", "id": id, "status": "done"})));
    }

    fn scroll_chat(&mut self, delta: i32) {
        let current = if self.chat_follow {
            self.chat_scroll_max
        } else {
            self.chat_scroll.min(self.chat_scroll_max)
        };
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs() as u16)
        } else {
            current.saturating_add(delta as u16)
        }
        .min(self.chat_scroll_max);
        self.chat_scroll = next;
        self.chat_follow = next >= self.chat_scroll_max;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.commands.send(RemoteCommand::Shutdown);
    }
}

// ── line editing ─────────────────────────────────────────────────────────

/// Insert `text` at `cursor` (byte offset), advancing the cursor.
fn insert_at(buf: &mut String, cursor: &mut usize, text: &str) {
    let at = (*cursor).min(buf.len());
    buf.insert_str(at, text);
    *cursor = at + text.len();
}

/// Shared readline-ish editing for single/multi-line buffers.
fn edit_buffer(buf: &mut String, cursor: &mut usize, key: &KeyEvent) {
    let shift_or_none = key.modifiers - KeyModifiers::SHIFT;
    match key.code {
        KeyCode::Char(c) if shift_or_none.is_empty() => {
            let at = (*cursor).min(buf.len());
            buf.insert(at, c);
            *cursor = at + c.len_utf8();
        }
        KeyCode::Backspace => {
            if *cursor > 0
                && let Some((start, _)) = prev_boundary(buf, *cursor)
            {
                buf.replace_range(start..*cursor, "");
                *cursor = start;
            }
        }
        KeyCode::Delete => {
            if *cursor < buf.len()
                && let Some((_, end)) = next_boundary(buf, *cursor)
            {
                buf.replace_range(*cursor..end, "");
            }
        }
        KeyCode::Left => {
            if let Some((start, _)) = prev_boundary(buf, *cursor) {
                *cursor = start;
            }
        }
        KeyCode::Right => {
            if let Some((_, end)) = next_boundary(buf, *cursor) {
                *cursor = end;
            }
        }
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = buf.len(),
        _ => {}
    }
}

/// (start, end) byte offsets of the char ending at `pos` / starting at `pos`.
fn prev_boundary(buf: &str, pos: usize) -> Option<(usize, usize)> {
    let pos = pos.min(buf.len());
    buf[..pos]
        .char_indices()
        .next_back()
        .map(|(i, c)| (i, i + c.len_utf8()))
}

fn next_boundary(buf: &str, pos: usize) -> Option<(usize, usize)> {
    let pos = pos.min(buf.len());
    buf[pos..]
        .char_indices()
        .next()
        .map(|(i, c)| (pos + i, pos + i + c.len_utf8()))
}
