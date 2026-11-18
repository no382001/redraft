use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use super::text::width;

pub struct Popup {
    title: String,
    lines: Vec<Line<'static>>,
    footer: String,
    top: usize,
    pub selected: Option<usize>,
}

impl Popup {
    pub fn new(title: &str, lines: Vec<Line<'static>>, footer: &str) -> Self {
        Self { title: title.to_string(), lines, footer: footer.to_string(), top: 0, selected: None }
    }

    pub fn with_selection(mut self, index: usize) -> Self {
        self.selected = Some(index.min(self.lines.len().saturating_sub(1)));
        self
    }

    fn rect(&self, area: Rect) -> Rect {
        let content = self.lines.iter().map(|l| l.width()).max().unwrap_or(0);
        let min = (width(&self.title).max(width(&self.footer)) + 6).max(36);
        let w = (content + 4).max(min).min(area.width.saturating_sub(4).max(10) as usize) as u16;
        let h = (self.lines.len() + 2).clamp(3, area.height.saturating_sub(2).max(3) as usize) as u16;
        let (w, h) = (w.min(area.width), h.min(area.height));
        Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h)
    }

    pub fn visible_rows(&self, area: Rect) -> usize {
        self.rect(area).height.saturating_sub(2) as usize
    }

    pub fn scroll_by(&mut self, delta: isize, area: Rect) {
        let max = self.lines.len().saturating_sub(self.visible_rows(area)) as isize;
        self.top = (self.top as isize).saturating_add(delta).clamp(0, max) as usize;
    }

    pub fn select_by(&mut self, delta: isize, area: Rect) {
        let Some(sel) = self.selected else { return };
        let max = self.lines.len().saturating_sub(1) as isize;
        let sel = (sel as isize).saturating_add(delta).clamp(0, max) as usize;
        self.selected = Some(sel);
        let visible = self.visible_rows(area).max(1);
        if sel < self.top {
            self.top = sel;
        }
        if sel >= self.top + visible {
            self.top = sel + 1 - visible;
        }
    }

    pub fn render(&self, f: &mut Frame) {
        let rect = self.rect(f.area());
        let border = Style::new().fg(Color::Cyan);
        let block = Block::bordered()
            .border_style(border)
            .title(Line::styled(format!(" {} ", self.title), border.add_modifier(Modifier::BOLD)))
            .title_bottom(Line::styled(format!(" {} ", self.footer), Style::new().add_modifier(Modifier::DIM)).right_aligned())
            .style(Style::new().bg(Color::Indexed(235)));

        let inner_width = rect.width.saturating_sub(4) as usize;
        let lines: Vec<Line> = self
            .lines
            .iter()
            .enumerate()
            .skip(self.top)
            .map(|(i, l)| {
                if self.selected == Some(i) {
                    let mut l = l.clone();
                    let pad = inner_width.saturating_sub(l.width());
                    l.push_span(" ".repeat(pad));
                    l.style(Style::new().bg(Color::Indexed(238)).add_modifier(Modifier::BOLD))
                } else {
                    l.clone()
                }
            })
            .collect();

        f.render_widget(Clear, rect);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let padded = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
        f.render_widget(Paragraph::new(lines), padded);
    }
}
