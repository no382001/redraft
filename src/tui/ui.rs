use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::app::{App, Pane};
use super::text::{fit, relative_date, width};

pub const ACCENT: Color = Color::Cyan;
pub const SELECTED: Style = Style::new().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);

pub fn pane_block(title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title);
    if focused {
        block
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(ACCENT))
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
    } else {
        block.border_type(BorderType::Plain).border_style(Style::new().fg(Color::DarkGray)).title_style(DIM)
    }
}

pub fn render(app: &App, f: &mut Frame) {
    let area = f.area();
    if area.height < 5 {
        return;
    }
    render_header(app, f, Rect::new(0, 0, area.width, 1));

    let list = app.list_rect();
    if list.width > 0 {
        render_list(app, f, list);
    }
    let diff = app.diff_rect();
    if diff.width > 0 {
        render_diff(app, f, diff);
    }

    render_footer(app, f, Rect::new(0, area.height - 1, area.width, 1));
}

fn render_header(app: &App, f: &mut Frame, rect: Rect) {
    let mut spans = vec![
        Span::styled(" redraft ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {}", app.repo_name)),
    ];
    if let Some(b) = &app.branch {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(b.clone(), Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)));
    }
    if let Some(r) = app.log.as_ref().and_then(|l| l.remote.as_ref()) {
        let mut parts = Vec::new();
        if let Some(a) = r.ahead.filter(|a| *a > 0) {
            parts.push(format!("{a} ahead"));
        }
        if let Some(b) = r.behind.filter(|b| *b > 0) {
            parts.push(format!("{b} behind"));
        }
        let (text, color) = if parts.is_empty() {
            (format!("in sync with {}", r.branch), Color::Green)
        } else {
            (format!("{} {}", parts.join(", "), r.branch), Color::Yellow)
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(text, Style::new().fg(color)));
    }

    let left_w: usize = spans.iter().map(|s| s.width()).sum();
    let right = if app.searching {
        Span::styled(format!("/{}▏ ", app.filter), Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else if !app.filter.is_empty() {
        Span::styled(format!("filter: {} ", app.filter), Style::new().fg(Color::Yellow))
    } else {
        Span::raw("")
    };
    let gap = (rect.width as usize).saturating_sub(left_w + right.width());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(right);
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}

fn render_list(app: &App, f: &mut Frame, rect: Rect) {
    let focused = app.focus == Pane::List;
    let count = match (&app.log, app.filter.is_empty()) {
        (Some(log), true) => format!(" Commits ({}) ", log.commits.len()),
        (Some(log), false) => format!(" Commits ({} of {}) ", app.filtered.len(), log.commits.len()),
        (None, _) => " Commits ".to_string(),
    };
    let block = pane_block(Line::raw(count), focused);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let w = inner.width as usize;
    let mut lines = Vec::with_capacity(inner.height as usize);
    if app.filtered.is_empty() {
        let msg = if app.filter.is_empty() { " No commits on this branch yet." } else { " No commits match your search." };
        lines.push(Line::styled(msg, DIM));
    }

    let now = chrono::Utc::now();
    for (i, c) in app.filtered.iter().enumerate().skip(app.list_top).take(inner.height as usize) {
        let selected = i == app.cursor;
        let author_w = if w >= 70 { 14 } else { 0 };
        let fixed = 2 + 7 + 1 + 4 + 2 + if author_w > 0 { author_w + 1 } else { 0 };
        let subject_w = w.saturating_sub(fixed);

        let badges = app.badges(c).join(" ");
        let badge_text = if badges.is_empty() { String::new() } else { format!("{badges} ") };
        let badge_w = width(&badge_text).min(subject_w);
        let marker = if selected && !focused { "▶ " } else { "  " };

        let mut spans = vec![
            Span::styled(marker, Style::new().fg(ACCENT)),
            Span::styled(c.short_sha.clone(), Style::new().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled(format!("{:>4}", relative_date(&c.author_date, now)), DIM),
            Span::raw("  "),
            Span::styled(fit(&badge_text, badge_w), Style::new().fg(Color::Magenta)),
            Span::styled(
                fit(c.subject(), subject_w - badge_w),
                match (selected, c.editable) {
                    (true, _) => Style::new().add_modifier(Modifier::BOLD),
                    (false, true) => Style::new(),
                    (false, false) => DIM,
                },
            ),
        ];
        if author_w > 0 {
            spans.push(Span::styled(format!(" {}", fit(&c.author_name, author_w)), Style::new().fg(Color::Blue)));
        }

        let line = if selected && focused {
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            Line::styled(fit(&text, w), SELECTED)
        } else {
            Line::from(spans)
        };
        lines.push(line);
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_diff(app: &App, f: &mut Frame, rect: Rect) {
    let focused = app.focus == Pane::Diff;
    let title = match app.selected() {
        Some(c) => {
            let total = app.diff_lines.len();
            let pos = if total == 0 { String::new() } else { format!(" · {}/{total}", (app.diff_top as usize + 1).min(total)) };
            format!(" Diff {}{pos} ", c.short_sha)
        }
        None => " Diff ".to_string(),
    };
    let block = pane_block(Line::raw(title), focused);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let padded = Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner };
    f.render_widget(Paragraph::new(app.diff_lines.clone()).scroll((app.diff_top, app.diff_left)), padded);
}

fn render_footer(app: &App, f: &mut Frame, rect: Rect) {
    let w = rect.width as usize;
    if let Some((text, is_error)) = &app.message {
        let color = if *is_error { Color::Red } else { Color::Green };
        let line = Line::styled(fit(&format!(" {text}"), w), Style::new().fg(color).add_modifier(Modifier::BOLD));
        f.render_widget(Paragraph::new(line), rect);
        return;
    }

    let hints: &[(&str, &str)] = if app.searching {
        &[("type", "filter"), ("enter", "keep"), ("esc", "clear")]
    } else if app.focus == Pane::List {
        &[("j/k", "move"), ("tab", "diff"), ("r", "reword"), ("e", "edit diff"), ("o", "pager"), ("b", "branch"), ("/", "search"), ("H", "history"), ("?", "help"), ("q", "quit")]
    } else {
        &[("j/k", "scroll"), ("h/l", "pan"), ("tab", "commits"), ("r", "reword"), ("e", "edit diff"), ("o", "pager"), ("?", "help"), ("q", "quit")]
    };

    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (key, label) in hints {
        let piece = width(key) + 1 + width(label) + 2;
        if used + piece > w {
            break;
        }
        spans.push(Span::styled(*key, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {label}  "), DIM));
        used += piece;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}
