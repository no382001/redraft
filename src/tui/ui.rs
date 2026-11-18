use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Pane};
use super::text::{fit, relative_date, width};

const HEADER_BG: Color = Color::Indexed(236);
const SELECTED_BG: Color = Color::Indexed(238);

pub fn render(app: &App, f: &mut Frame) {
    let area = f.area();
    if area.height < 3 {
        return;
    }
    render_header(app, f, Rect::new(0, 0, area.width, 1));

    let list = app.list_rect();
    let diff = app.diff_rect();
    if list.width > 0 {
        render_list(app, f, list);
    }
    if list.width > 0 && diff.width > 0 {
        let sep = Rect::new(list.width, list.y, 1, list.height);
        let bar = vec![Line::raw("│"); sep.height as usize];
        f.render_widget(Paragraph::new(bar).style(Style::new().add_modifier(Modifier::DIM)), sep);
    }
    if diff.width > 0 {
        let pane = Rect { x: diff.x + 1, width: diff.width.saturating_sub(1), ..diff };
        f.render_widget(Paragraph::new(app.diff_lines.clone()).scroll((app.diff_top, app.diff_left)), pane);
    }

    render_footer(app, f, Rect::new(0, area.height - 1, area.width, 1));
}

fn render_header(app: &App, f: &mut Frame, rect: Rect) {
    let mut left = format!(" gitkit  {}", app.repo_name);
    if let Some(b) = &app.branch {
        left.push_str(&format!("  ⎇ {b}"));
    }
    if let Some(log) = &app.log {
        let n = log.commits.len();
        left.push_str(&format!("  ·  {n} commit{}", if n == 1 { "" } else { "s" }));
        if let Some(r) = &log.remote {
            let mut parts = Vec::new();
            if let Some(a) = r.ahead.filter(|a| *a > 0) {
                parts.push(format!("{a} ahead"));
            }
            if let Some(b) = r.behind.filter(|b| *b > 0) {
                parts.push(format!("{b} behind"));
            }
            let state = if parts.is_empty() { "up to date with".to_string() } else { parts.join(", ") };
            left.push_str(&format!(" · {state} {}", r.branch));
        }
    }

    let right = if app.searching {
        format!(" /{}▏ ", app.filter)
    } else if !app.filter.is_empty() {
        format!(" filter: {} ({}) ", app.filter, app.filtered.len())
    } else {
        String::new()
    };
    let right_w = width(&right).min(rect.width as usize / 2);
    let left_w = (rect.width as usize).saturating_sub(right_w);

    let line = Line::from(vec![
        Span::styled(fit(&left, left_w), Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(fit(&right, right_w), Style::new().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(line).style(Style::new().bg(HEADER_BG)), rect);
}

fn render_list(app: &App, f: &mut Frame, rect: Rect) {
    let w = rect.width as usize;
    let mut lines = Vec::with_capacity(rect.height as usize);

    if app.filtered.is_empty() {
        let msg = if app.filter.is_empty() { "  No commits on this branch yet." } else { "  No commits match your search." };
        lines.push(Line::raw(""));
        lines.push(Line::styled(msg, Style::new().add_modifier(Modifier::DIM)));
    }

    let now = chrono::Utc::now();
    for (i, c) in app.filtered.iter().enumerate().skip(app.list_top).take(rect.height as usize) {
        let selected = i == app.cursor;
        let author_w = if w >= 72 { 16 } else { 0 };
        let fixed = 1 + 7 + 1 + 4 + 2 + if author_w > 0 { author_w + 1 } else { 0 };
        let subject_w = w.saturating_sub(fixed);

        let badges = app.badges(c).join(" ");
        let badge_text = if badges.is_empty() { String::new() } else { format!("{badges} ") };
        let badge_w = width(&badge_text).min(subject_w);
        let bold = if selected { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() };

        let mut spans = vec![
            Span::raw(" "),
            Span::styled(c.short_sha.clone(), Style::new().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled(format!("{:>4}", relative_date(&c.author_date, now)), Style::new().add_modifier(Modifier::DIM)),
            Span::raw("  "),
            Span::styled(fit(&badge_text, badge_w), Style::new().fg(Color::Cyan)),
            Span::styled(fit(c.subject(), subject_w - badge_w), bold),
        ];
        if author_w > 0 {
            spans.push(Span::styled(format!(" {}", fit(&c.author_name, author_w)), Style::new().fg(Color::Blue)));
        }

        let mut line = Line::from(spans);
        if selected {
            let bg = if app.focus == Pane::List { SELECTED_BG } else { HEADER_BG };
            let pad = w.saturating_sub(line.width());
            line.push_span(" ".repeat(pad));
            line = line.style(Style::new().bg(bg));
        }
        lines.push(line);
    }

    f.render_widget(Paragraph::new(lines), rect);
}

fn render_footer(app: &App, f: &mut Frame, rect: Rect) {
    let w = rect.width as usize;
    let line = match &app.message {
        Some((text, is_error)) => {
            let color = if *is_error { Color::Red } else { Color::Green };
            Line::styled(fit(&format!(" {text}"), w), Style::new().fg(color).add_modifier(Modifier::BOLD))
        }
        None => {
            let hints = if app.searching {
                " type to filter · enter keep · esc clear"
            } else if app.focus == Pane::List {
                " ↑↓ move · enter diff · r reword · e edit diff · o pager · b branch · / search · H history · ? help · q quit"
            } else {
                " ↑↓ scroll · ←→ pan · esc back · r reword · e edit diff · o pager · ? help · q quit"
            };
            Line::styled(fit(hints, w), Style::new().add_modifier(Modifier::DIM))
        }
    };
    f.render_widget(Paragraph::new(line), rect);
}
