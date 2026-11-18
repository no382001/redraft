use std::io::{Write, stdout};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use redraft::model::{CommitInfo, DiffConflict, Log, Resolutions, Rewrite};
use redraft::{Error, GitService};

use super::external::External;
use super::popup::Popup;
use super::text::{clean_message, has_conflict_markers, is_conflict_marker, sanitize};
use super::ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Diff,
}

pub struct App {
    pub git: GitService,
    pub external: External,
    pub repo_name: String,

    pub branch: Option<String>,
    pub log: Option<Log>,
    pub filtered: Vec<CommitInfo>,
    pub filter: String,
    pub searching: bool,

    pub cursor: usize,
    pub list_top: usize,
    pub focus: Pane,
    pub diff_top: u16,
    pub diff_left: u16,
    pub diff_sha: Option<String>,
    pub diff_lines: Vec<Line<'static>>,
    pub split: f32,

    pub message: Option<(String, bool)>,
    pub area: Rect,
    dragging_split: bool,
    quit: bool,
}

type Outcome<T> = Result<T, Error>;

impl App {
    pub fn new(git: GitService) -> Self {
        let repo_name = git.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let external = External::new(git.path());
        Self {
            git,
            external,
            repo_name,
            branch: None,
            log: None,
            filtered: Vec::new(),
            filter: String::new(),
            searching: false,
            cursor: 0,
            list_top: 0,
            focus: Pane::List,
            diff_top: 0,
            diff_left: 0,
            diff_sha: None,
            diff_lines: Vec::new(),
            split: 0.45,
            message: None,
            area: Rect::default(),
            dragging_split: false,
            quit: false,
        }
    }

    pub fn run(mut self, term: &mut DefaultTerminal) -> std::io::Result<()> {
        match self.git.branches() {
            Ok(b) => {
                self.branch = b.current.or_else(|| b.names.first().cloned());
                self.load_log(None);
            }
            Err(e) => self.error(format!("Couldn't load repository: {e}")),
        }

        while !self.quit {
            self.draw(term, None)?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(term, key),
                Event::Mouse(mouse) => self.on_mouse(mouse),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn draw(&mut self, term: &mut DefaultTerminal, popup: Option<&Popup>) -> std::io::Result<()> {
        self.ensure_diff();
        let size = term.size()?;
        self.area = Rect::new(0, 0, size.width, size.height);
        self.clamp_list();
        term.draw(|f| {
            ui::render(self, f);
            if let Some(p) = popup {
                p.render(f);
            }
        })?;
        Ok(())
    }

    pub fn selected(&self) -> Option<&CommitInfo> {
        self.filtered.get(self.cursor)
    }

    fn load_log(&mut self, rewrite: Option<&Rewrite>) {
        let mut keep = self.selected().map(|c| c.sha.clone());
        if let (Some(rw), Some(sha)) = (rewrite, &keep) {
            if let Some(new) = rw.remap.get(sha) {
                keep = Some(new.clone());
            }
        }

        match self.git.log(self.branch.as_deref()) {
            Ok(log) => self.log = Some(log),
            Err(e) => {
                self.log = None;
                self.error(format!("Couldn't load history: {e}"));
            }
        }
        self.diff_sha = None;
        self.apply_filter(keep);
    }

    fn apply_filter(&mut self, keep: Option<String>) {
        let keep = keep.or_else(|| self.selected().map(|c| c.sha.clone()));
        let q = self.filter.trim().to_lowercase();
        let commits = self.log.as_ref().map(|l| l.commits.as_slice()).unwrap_or_default();
        self.filtered = commits
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.message.to_lowercase().contains(&q)
                    || c.sha.starts_with(&q)
                    || c.author_name.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();

        if let Some(i) = keep.and_then(|k| self.filtered.iter().position(|c| c.sha == k)) {
            self.cursor = i;
        }
        self.clamp_list();
    }

    fn ensure_diff(&mut self) {
        let sha = self.selected().map(|c| c.sha.clone());
        if sha == self.diff_sha {
            return;
        }
        self.diff_top = 0;
        self.diff_left = 0;
        self.diff_lines = match self.selected() {
            Some(c) => self.build_detail(c),
            None => Vec::new(),
        };
        self.diff_sha = sha;
    }

    fn build_detail(&self, c: &CommitInfo) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let badges = self.badges(c).join(" ");
        lines.push(Line::from(vec![
            Span::styled(format!("commit {}", c.sha), Style::new().fg(Color::Yellow)),
            Span::styled(if badges.is_empty() { String::new() } else { format!("  {badges}") }, Style::new().fg(Color::Cyan)),
        ]));
        lines.push(Line::raw(sanitize(&format!("Author: {} <{}>", c.author_name, c.author_email))));
        let age = super::text::relative_date(&c.author_date, chrono::Utc::now());
        let age = if age == "now" { "just now".to_string() } else { format!("{age} ago") };
        lines.push(Line::raw(format!("Date:   {} ({age})", c.author_date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"))));
        if c.is_merge {
            lines.push(Line::styled("Merge:  diff shown against the first parent", Style::new().add_modifier(Modifier::DIM)));
        }
        lines.push(Line::raw(""));
        for l in c.message.trim_end_matches('\n').split('\n') {
            lines.push(Line::raw(format!("    {}", sanitize(l))));
        }
        lines.push(Line::raw(""));

        match self.git.diff(&c.sha) {
            Ok(diff) if diff.is_empty() => lines.push(Line::styled("(no changes)", Style::new().add_modifier(Modifier::DIM))),
            Ok(diff) => lines.extend(diff.trim_end_matches('\n').split('\n').map(|l| Line::styled(sanitize(l), diff_style(l)))),
            Err(e) => lines.push(Line::styled(format!("Couldn't load diff: {e}"), Style::new().fg(Color::Red))),
        }
        lines
    }

    pub fn badges(&self, c: &CommitInfo) -> Vec<String> {
        let mut b = Vec::new();
        if c.is_merge {
            b.push("[merge]".to_string());
        }
        if let Some(remote) = self.log.as_ref().and_then(|l| l.remote.as_ref()) {
            if remote.sha.as_deref() == Some(c.sha.as_str()) {
                b.push(format!("[{}]", remote.branch));
            }
        }
        b
    }

    pub fn body_height(&self) -> usize {
        self.area.height.saturating_sub(4) as usize
    }

    pub fn two_panes(&self) -> bool {
        self.area.width >= 90
    }

    pub fn list_width(&self) -> u16 {
        let w = self.area.width;
        ((w as f32 * self.split) as u16).clamp(30, w.saturating_sub(31).max(30))
    }

    fn clamp_list(&mut self) {
        self.cursor = self.cursor.min(self.filtered.len().saturating_sub(1));
        let h = self.body_height().max(1);
        if self.cursor < self.list_top {
            self.list_top = self.cursor;
        }
        if self.cursor >= self.list_top + h {
            self.list_top = self.cursor + 1 - h;
        }
        self.list_top = self.list_top.min(self.filtered.len().saturating_sub(h));
    }

    fn on_key(&mut self, term: &mut DefaultTerminal, key: KeyEvent) {
        self.message = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.searching {
            self.on_search_key(key);
            return;
        }

        let page = self.body_height().saturating_sub(1).max(1) as isize;
        match key.code {
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            KeyCode::Enter if self.focus == Pane::List => self.focus = Pane::Diff,
            KeyCode::Esc => {
                if self.focus == Pane::Diff {
                    self.focus = Pane::List;
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.apply_filter(None);
                }
            }
            KeyCode::Left => self.pan(-8),
            KeyCode::Right => self.pan(8),
            KeyCode::F(5) => self.refresh(),
            KeyCode::Char('d') if ctrl => self.move_by(page / 2),
            KeyCode::Char('u') if ctrl => self.move_by(-page / 2),
            KeyCode::Char('r') if ctrl => self.refresh(),
            _ if ctrl => {}
            KeyCode::Char(c) => match c {
                'q' => self.quit = true,
                'j' => self.move_by(1),
                'k' => self.move_by(-1),
                ' ' => self.move_by(page),
                'g' => self.move_by(isize::MIN / 2),
                'G' => self.move_by(isize::MAX / 2),
                'h' => self.pan(-8),
                'l' => self.pan(8),
                'n' => self.select_by(1),
                'p' => self.select_by(-1),
                'z' => self.toggle_focus(),
                '/' => self.searching = true,
                'r' => self.reword(term),
                'e' => self.edit_diff(term),
                'o' => self.open_in_pager(term),
                'b' => self.pick_branch(term),
                'H' => self.show_history(term),
                'R' => self.refresh(),
                'y' => self.yank_sha(),
                '<' => self.split = (self.split - 0.05).max(0.2),
                '>' => self.split = (self.split + 0.05).min(0.8),
                '?' => self.show_help(term),
                _ => {}
            },
            _ => {}
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.searching = false,
            KeyCode::Esc => {
                self.searching = false;
                self.filter.clear();
                self.apply_filter(None);
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter(None);
            }
            KeyCode::Up => self.select_by(-1),
            KeyCode::Down => self.select_by(1),
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.apply_filter(None);
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let pos = Position::new(mouse.column, mouse.row);
        let in_list = self.list_rect().contains(pos);
        let in_diff = self.diff_rect().contains(pos);

        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                if in_list {
                    self.select_by(delta);
                } else if in_diff {
                    self.scroll_diff(delta);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list = self.list_rect();
                let on_divider = mouse.column + 1 == list.right() || mouse.column == list.right();
                if self.two_panes() && on_divider && list.contains(Position::new(list.x, mouse.row)) {
                    self.dragging_split = true;
                } else if in_list {
                    self.focus = Pane::List;
                    if mouse.row > list.y && mouse.row + 1 < list.bottom() {
                        let row = (mouse.row - list.y - 1) as usize + self.list_top;
                        if row < self.filtered.len() {
                            self.cursor = row;
                        }
                    }
                } else if in_diff {
                    self.focus = Pane::Diff;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging_split && self.area.width > 0 => {
                self.split = (mouse.column as f32 / self.area.width as f32).clamp(0.2, 0.8);
            }
            MouseEventKind::Up(_) => self.dragging_split = false,
            _ => {}
        }
    }

    pub fn list_rect(&self) -> Rect {
        let body = Rect::new(0, 1, self.area.width, self.area.height.saturating_sub(2));
        match (self.two_panes(), self.focus) {
            (true, _) => Rect { width: self.list_width(), ..body },
            (false, Pane::List) => body,
            (false, Pane::Diff) => Rect { width: 0, ..body },
        }
    }

    pub fn diff_rect(&self) -> Rect {
        let body = Rect::new(0, 1, self.area.width, self.area.height.saturating_sub(2));
        match (self.two_panes(), self.focus) {
            (true, _) => {
                let x = self.list_width();
                Rect { x, width: body.width.saturating_sub(x), ..body }
            }
            (false, Pane::Diff) => body,
            (false, Pane::List) => Rect { width: 0, ..body },
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = if self.focus == Pane::List { Pane::Diff } else { Pane::List };
    }

    fn move_by(&mut self, delta: isize) {
        match self.focus {
            Pane::List => self.select_by(delta),
            Pane::Diff => self.scroll_diff(delta),
        }
    }

    fn select_by(&mut self, delta: isize) {
        let max = self.filtered.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize).saturating_add(delta).clamp(0, max) as usize;
        self.clamp_list();
    }

    fn scroll_diff(&mut self, delta: isize) {
        let max = self.diff_lines.len().saturating_sub(self.body_height()) as isize;
        self.diff_top = (self.diff_top as isize).saturating_add(delta).clamp(0, max) as u16;
    }

    fn pan(&mut self, delta: i32) {
        if self.focus == Pane::Diff && (delta > 0 || self.diff_left > 0) {
            self.diff_left = (self.diff_left as i32 + delta).max(0) as u16;
        } else {
            self.focus = if delta > 0 { Pane::Diff } else { Pane::List };
        }
    }

    fn refresh(&mut self) {
        self.load_log(None);
        if self.message.is_none() {
            self.info("Reloaded.");
        }
    }

    fn reword(&mut self, term: &mut DefaultTerminal) {
        let (Some(c), Some(branch)) = (self.selected().cloned(), self.branch.clone()) else { return };

        let template = format!(
            "{}\n\n\
             # Rewording {} on {branch}. Lines starting with '#' are ignored,\n\
             # and an empty message aborts the reword. Every later commit on the branch\n\
             # is rebuilt (new shas, same trees); the old tip is kept under refs/redraft-backup/.\n",
            c.message.trim_end_matches('\n'),
            c.short_sha
        );
        let Some(edited) = self.external.edit(term, &template, "COMMIT_EDITMSG") else {
            return self.error("Editor exited with an error; reword aborted.");
        };

        let message = clean_message(&edited);
        if message.is_empty() {
            return self.error("Empty message; reword aborted.");
        }
        if message == clean_message(&c.message) {
            return self.info("Message unchanged.");
        }

        self.busy(term, format!("Rewording {}…", c.short_sha));
        match self.git.reword(&c.sha, &message, Some(&branch)) {
            Ok(result) => self.rewritten(&result),
            Err(e) => self.error(format!("Reword failed: {e}")),
        }
    }

    fn edit_diff(&mut self, term: &mut DefaultTerminal) {
        let (Some(c), Some(branch)) = (self.selected().cloned(), self.branch.clone()) else { return };

        let original = match self.git.diff(&c.sha) {
            Ok(d) => d,
            Err(e) => return self.error(e.to_string()),
        };
        if original.is_empty() {
            return self.info("This commit has no changes to edit.");
        }

        let mut text = original.clone();
        loop {
            let Some(edited) = self.external.edit(term, &text, &format!("{}.diff", c.short_sha)) else {
                return self.error("Editor exited with an error; diff edit aborted.");
            };
            if edited == original {
                return self.info("Diff unchanged.");
            }
            text = edited;

            match self.apply_diff(term, &c, &text, &branch) {
                Ok(()) => return,
                Err(e) => {
                    let choice = self.ask(
                        term,
                        "Couldn't apply the edited diff",
                        &[e.to_string()],
                        &[('e', "edit the diff again (your edits are kept)"), ('q', "discard your edits")],
                    );
                    if choice != Some('e') {
                        return self.info("Diff edit discarded; nothing was changed.");
                    }
                }
            }
        }
    }

    fn apply_diff(&mut self, term: &mut DefaultTerminal, c: &CommitInfo, diff: &str, branch: &str) -> Outcome<()> {
        let mut resolutions = Resolutions::new();
        let mut auto_resolve = false;
        loop {
            self.busy(term, format!("Applying edited diff to {}…", c.short_sha));
            match self.git.edit_diff(&c.sha, diff, Some(branch), &resolutions, auto_resolve) {
                Ok(result) => {
                    self.rewritten(&result);
                    return Ok(());
                }
                Err(Error::Conflict(conflict)) => {
                    if !self.resolve_conflict(term, &conflict, &mut resolutions, &mut auto_resolve) {
                        self.info("Diff edit aborted; nothing was changed.");
                        return Ok(());
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn resolve_conflict(
        &mut self,
        term: &mut DefaultTerminal,
        conflict: &DiffConflict,
        resolutions: &mut Resolutions,
        auto_resolve: &mut bool,
    ) -> bool {
        self.info(format!("Resolving a conflict in {}…", conflict.commit_short_sha));
        let total = conflict.files.len();

        for (i, file) in conflict.files.iter().enumerate() {
            let theirs_label = format!("theirs: the file as {} has it", conflict.commit_short_sha);
            let mut options: Vec<(char, &str)> = vec![
                ('u', "unify: keep both sides, merged line by line"),
                ('o', "yours: the file as it is after your edit"),
                ('t', &theirs_label),
                ('e', "edit the unified version in your editor"),
                ('m', "edit with conflict markers in your editor"),
                ('v', "view the conflict"),
            ];
            if !*auto_resolve {
                options.push(('A', "auto-resolve every cleanly mergeable file from here on"));
            }
            options.push(('q', "abort the whole diff edit"));

            let resolved = loop {
                let body = [
                    format!("Replaying \"{}\" on top of your edit conflicted.", conflict.commit_subject),
                    String::new(),
                    format!("File {} of {total}: {}", i + 1, file.path),
                ];
                let choice = self.ask(term, &format!("Conflict in {}", conflict.commit_short_sha), &body, &options);
                let picked = match choice {
                    Some('u') => Some(file.unified.clone()),
                    Some('o') => Some(file.ours.clone()),
                    Some('t') => Some(file.theirs.clone()),
                    Some('e') => self.edit_resolution(term, &file.unified, &file.path),
                    Some('m') => self.edit_resolution(term, &file.with_markers, &file.path),
                    Some('v') => {
                        let lines = file
                            .with_markers
                            .trim_end_matches('\n')
                            .split('\n')
                            .map(|l| {
                                let style = if is_conflict_marker(l) {
                                    Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)
                                } else {
                                    Style::new()
                                };
                                Line::styled(sanitize(l), style)
                            })
                            .collect();
                        self.show_text(term, &file.path, lines);
                        None
                    }
                    Some('A') => {
                        *auto_resolve = true;
                        return true;
                    }
                    _ => return false,
                };
                if let Some(text) = picked {
                    break text;
                }
            };

            resolutions.entry(conflict.commit_sha.clone()).or_default().insert(file.path.clone(), resolved);
        }
        true
    }

    fn edit_resolution(&mut self, term: &mut DefaultTerminal, text: &str, path: &str) -> Option<String> {
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let mut text = text.to_string();
        loop {
            let edited = self.external.edit(term, &text, &name)?;
            if !has_conflict_markers(&edited) {
                return Some(edited);
            }
            let choice = self.ask(
                term,
                "Conflict markers remain",
                &[format!("{path} still contains <<<<<<< / ======= / >>>>>>> lines.")],
                &[('e', "edit again"), ('k', "keep it as it is"), ('b', "back to the choices")],
            );
            match choice {
                Some('k') => return Some(edited),
                Some('e') => text = edited,
                _ => return None,
            }
        }
    }

    fn rewritten(&mut self, result: &Rewrite) {
        self.load_log(Some(result));
        if let Some(w) = &result.warning {
            return self.error(w.clone());
        }
        let mut msg = format!("Rewrote {} commit(s).", result.remap.len());
        if !result.auto_resolved_files.is_empty() {
            msg.push_str(&format!(" Auto-resolved {} file(s).", result.auto_resolved_files.len()));
        }
        if !result.updated_working_files.is_empty() {
            msg.push_str(&format!(" Updated {} working file(s).", result.updated_working_files.len()));
        }
        msg.push_str(&format!(" Backup: {}. Already pushed? Force-push with --force-with-lease.", result.backup_ref));
        self.info(msg);
    }

    fn open_in_pager(&mut self, term: &mut DefaultTerminal) {
        if let Some(c) = self.selected().cloned() {
            self.external.show(term, &c.sha);
        }
    }

    fn pick_branch(&mut self, term: &mut DefaultTerminal) {
        let branches = match self.git.branches() {
            Ok(b) => b,
            Err(e) => return self.error(e.to_string()),
        };
        if branches.names.is_empty() {
            return self.info("No local branches.");
        }
        let items: Vec<String> = branches
            .names
            .iter()
            .map(|b| format!("{}{b}", if Some(b) == branches.current.as_ref() { "* " } else { "  " }))
            .collect();
        let initial = branches.names.iter().position(|b| Some(b) == self.branch.as_ref()).unwrap_or(0);
        let Some(i) = self.choose(term, "Branch", items, initial) else { return };
        if Some(&branches.names[i]) == self.branch.as_ref() {
            return;
        }
        self.branch = Some(branches.names[i].clone());
        self.cursor = 0;
        self.list_top = 0;
        self.load_log(None);
    }

    fn show_history(&mut self, term: &mut DefaultTerminal) {
        let entries = match self.git.audit_log() {
            Ok(e) => e,
            Err(e) => return self.error(format!("Couldn't load history: {e}")),
        };
        let s = |e: &serde_json::Map<String, serde_json::Value>, k: &str| e.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let short = |sha: Option<String>| sha.map(|s| s.chars().take(7).collect()).unwrap_or_else(|| "?".to_string());

        let mut lines = Vec::new();
        if entries.is_empty() {
            lines.push(Line::styled("No history yet.", Style::new().add_modifier(Modifier::DIM)));
        }
        for e in &entries {
            let when = s(e, "timestampUtc")
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            lines.push(Line::styled(
                format!("{} on {} — {when}", s(e, "kind").unwrap_or("reword".into()), s(e, "branch").unwrap_or("?".into())),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(
                format!("  {} → new tip {}", short(s(e, "targetSha")), short(s(e, "newHeadSha"))),
                Style::new().fg(Color::Yellow),
            ));
            if let (Some(old), Some(new)) = (s(e, "oldMessage"), s(e, "newMessage")) {
                if old != new {
                    lines.push(Line::raw(sanitize(&format!(
                        "  \"{}\" → \"{}\"",
                        redraft::model::first_line(&old),
                        redraft::model::first_line(&new)
                    ))));
                }
            }
            lines.push(Line::styled(format!("  backup: {}", s(e, "backupRef").unwrap_or("—".into())), Style::new().add_modifier(Modifier::DIM)));
            lines.push(Line::raw(""));
        }
        self.show_text(term, "Activity history", lines);
    }

    fn yank_sha(&mut self) {
        let Some(c) = self.selected() else { return };
        let sha = c.sha.clone();
        let encoded = base64(sha.as_bytes());
        let _ = write!(stdout(), "\x1b]52;c;{encoded}\x07");
        let _ = stdout().flush();
        self.info(format!("Copied {sha} to the clipboard."));
    }

    fn show_help(&mut self, term: &mut DefaultTerminal) {
        const HELP: &[&str] = &[
            "Navigation",
            "  ↑ ↓  j k        move selection / scroll diff",
            "  PgUp PgDn space page            g G  Home End   top / bottom",
            "  Ctrl+D Ctrl+U   half page",
            "  enter tab → l   focus the diff        esc ← h   back to the list",
            "  ← → h l         pan the diff sideways when it has focus",
            "  n p             next / previous commit from anywhere",
            "  < >             resize the panes (or drag the divider)",
            "  mouse           click to select, wheel to scroll",
            "",
            "Actions",
            "  r               reword the selected commit's message in your editor",
            "  e               edit the selected commit's diff in your editor",
            "  o               open the commit in git's pager (less, delta, …)",
            "  b               switch branch",
            "  /               filter commits by message, sha or author (esc clears)",
            "  H               activity history (every rewrite and its backup ref)",
            "  y               copy the full sha to the clipboard",
            "  R F5 Ctrl+R     reload from disk",
            "  q Ctrl+C        quit",
            "",
            "Rewrites rebuild every later commit on the branch. The old tip is always kept",
            "under refs/redraft-backup/, so `git reset --hard <backup ref>` undoes one.",
            "The editor comes from GIT_EDITOR, core.editor, VISUAL or EDITOR, like git.",
        ];
        let lines = HELP
            .iter()
            .map(|l| {
                if !l.is_empty() && !l.starts_with(' ') {
                    Line::styled(*l, Style::new().add_modifier(Modifier::BOLD))
                } else {
                    Line::raw(*l)
                }
            })
            .collect();
        self.show_text(term, "Help", lines);
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), true));
    }

    fn busy(&mut self, term: &mut DefaultTerminal, text: String) {
        self.info(text);
        let _ = self.draw(term, None);
    }

    fn show_text(&mut self, term: &mut DefaultTerminal, title: &str, lines: Vec<Line<'static>>) {
        let mut popup = Popup::new(title, lines, "↑↓ scroll · q close");
        loop {
            if self.draw(term, Some(&popup)).is_err() {
                return;
            }
            let Some(key) = next_key() else { continue };
            let visible = popup.visible_rows(self.area) as isize;
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?' | 'H') => return,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return,
                KeyCode::Up | KeyCode::Char('k') => popup.scroll_by(-1, self.area),
                KeyCode::Down | KeyCode::Char('j') => popup.scroll_by(1, self.area),
                KeyCode::PageUp => popup.scroll_by(-visible, self.area),
                KeyCode::PageDown | KeyCode::Char(' ') => popup.scroll_by(visible, self.area),
                KeyCode::Home | KeyCode::Char('g') => popup.scroll_by(isize::MIN / 2, self.area),
                KeyCode::End | KeyCode::Char('G') => popup.scroll_by(isize::MAX / 2, self.area),
                _ => {}
            }
        }
    }

    fn choose(&mut self, term: &mut DefaultTerminal, title: &str, items: Vec<String>, initial: usize) -> Option<usize> {
        let lines = items.into_iter().map(Line::raw).collect();
        let mut popup = Popup::new(title, lines, "enter select · esc cancel").with_selection(initial);
        loop {
            self.draw(term, Some(&popup)).ok()?;
            let key = next_key()?;
            let visible = popup.visible_rows(self.area) as isize;
            match key.code {
                KeyCode::Enter => return popup.selected,
                KeyCode::Esc | KeyCode::Char('q') => return None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
                KeyCode::Up | KeyCode::Char('k') => popup.select_by(-1, self.area),
                KeyCode::Down | KeyCode::Char('j') => popup.select_by(1, self.area),
                KeyCode::PageUp => popup.select_by(-visible, self.area),
                KeyCode::PageDown => popup.select_by(visible, self.area),
                _ => {}
            }
        }
    }

    fn ask(&mut self, term: &mut DefaultTerminal, title: &str, body: &[String], options: &[(char, &str)]) -> Option<char> {
        loop {
            let wrap_cols = (self.area.width as usize).saturating_sub(8).clamp(20, 76);
            let mut lines: Vec<Line<'static>> =
                body.iter().flat_map(|b| super::text::wrap(b, wrap_cols)).map(Line::raw).collect();
            lines.push(Line::raw(""));
            lines.extend(options.iter().map(|(k, label)| Line::styled(format!(" [{k}] {label}"), Style::new().fg(Color::Yellow))));
            let popup = Popup::new(title, lines, "press a key · esc cancel");

            self.draw(term, Some(&popup)).ok()?;
            let key = next_key()?;
            match key.code {
                KeyCode::Esc => return None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
                KeyCode::Char(c) if options.iter().any(|(k, _)| *k == c) => return Some(c),
                _ => {}
            }
        }
    }
}

fn next_key() -> Option<KeyEvent> {
    match event::read().ok()? {
        Event::Key(k) if k.kind == KeyEventKind::Press => Some(k),
        _ => None,
    }
}

pub fn diff_style(line: &str) -> Style {
    if line.starts_with("diff --git") {
        Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if line.starts_with("+++") || line.starts_with("---") {
        Style::new().add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::new().fg(Color::Cyan)
    } else if line.starts_with('+') {
        Style::new().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::new().fg(Color::Red)
    } else if line.starts_with('\\') {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_encodes() {
        assert_eq!(super::base64(b"abc"), "YWJj");
        assert_eq!(super::base64(b"ab"), "YWI=");
        assert_eq!(super::base64(b"a"), "YQ==");
    }
}
