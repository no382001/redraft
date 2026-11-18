use chrono::{DateTime, FixedOffset, Utc};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn sanitize(s: &str) -> String {
    if !s.chars().any(char::is_control) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn fit(s: &str, cols: usize) -> String {
    let s = sanitize(s);
    let mut out = String::new();
    let mut used = 0;
    if width(&s) <= cols {
        out.push_str(&s);
        used = width(&s);
    } else if cols > 0 {
        for c in s.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > cols - 1 {
                break;
            }
            out.push(c);
            used += w;
        }
        out.push('…');
        used += 1;
    }
    out.extend(std::iter::repeat_n(' ', cols.saturating_sub(used)));
    out
}

pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let raw = sanitize(raw);
        let mut current = String::new();
        for word in raw.split(' ') {
            if !current.is_empty() && width(&current) + 1 + width(word) > cols {
                lines.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(current);
    }
    lines
}

pub fn relative_date(when: &DateTime<FixedOffset>, now: DateTime<Utc>) -> String {
    let s = (now - when.with_timezone(&Utc)).num_seconds();
    const DAY: i64 = 86_400;
    if s < 60 {
        "now".into()
    } else if s < 3_600 {
        format!("{}m", s / 60)
    } else if s < DAY {
        format!("{}h", s / 3_600)
    } else if s < DAY * 14 {
        format!("{}d", s / DAY)
    } else if s < DAY * 60 {
        format!("{}w", s / (DAY * 7))
    } else if s < DAY * 365 {
        format!("{}mo", s / (DAY * 30))
    } else {
        format!("{}y", s / (DAY * 365))
    }
}

pub fn clean_message(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for raw in text.split('\n') {
        if raw.starts_with('#') {
            continue;
        }
        let line = raw.trim_end();
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" }
}

pub fn is_conflict_marker(line: &str) -> bool {
    line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> ") || line.starts_with("||||||| ") || line.trim_end() == "======="
}

pub fn has_conflict_markers(text: &str) -> bool {
    text.lines().any(is_conflict_marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fit_pads_truncates_and_handles_wide_chars() {
        assert_eq!(fit("abc", 5), "abc  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("世界世界", 5), "世界…");
        assert_eq!(width(&fit("世界世界", 6)), 6);
        assert_eq!(fit("a\tb\r", 6), "a    b");
        assert_eq!(fit("x", 0), "");
    }

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
    }

    #[test]
    fn relative_dates() {
        let now = Utc.with_ymd_and_hms(2026, 1, 31, 12, 0, 0).unwrap();
        let at = |secs: i64| (now - chrono::Duration::seconds(secs)).fixed_offset();
        assert_eq!(relative_date(&at(5), now), "now");
        assert_eq!(relative_date(&at(5 * 60), now), "5m");
        assert_eq!(relative_date(&at(3 * 3600), now), "3h");
        assert_eq!(relative_date(&at(2 * 86_400), now), "2d");
        assert_eq!(relative_date(&at(21 * 86_400), now), "3w");
        assert_eq!(relative_date(&at(90 * 86_400), now), "3mo");
        assert_eq!(relative_date(&at(800 * 86_400), now), "2y");
    }

    #[test]
    fn clean_message_strips_like_git() {
        assert_eq!(clean_message("\n\nsubject  \n\n\n\nbody\n# comment\n\n"), "subject\n\nbody\n");
        assert_eq!(clean_message("# only comments\n\n"), "");
    }

    #[test]
    fn detects_conflict_markers() {
        assert!(has_conflict_markers("a\n<<<<<<< yours\nb\n=======\nc\n>>>>>>> x\n"));
        assert!(!has_conflict_markers("a\n======== not a marker\n"));
    }
}
