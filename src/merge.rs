pub struct MergeResult {
    pub text: String,
    pub has_conflict: bool,
}

pub fn merge(ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
    let ancestor = split_lines(ancestor);
    let ours = split_lines(ours);
    let theirs = split_lines(theirs);

    let (ours_keep, ours_ins) = align_to_ancestor(&ancestor, &ours);
    let (theirs_keep, theirs_ins) = align_to_ancestor(&ancestor, &theirs);

    let mut result: Vec<&str> = Vec::new();
    let mut has_conflict = false;

    for i in 0..=ancestor.len() {
        result.extend(&ours_ins[i]);
        result.extend(&theirs_ins[i]);

        if i == ancestor.len() {
            break;
        }

        match (ours_keep[i], theirs_keep[i]) {
            (true, true) => result.push(ancestor[i]),
            (false, false) => has_conflict = true,
            _ => {}
        }
    }

    MergeResult { text: join_lines(&result), has_conflict }
}

pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

pub(crate) fn join_lines<S: AsRef<str>>(lines: &[S]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for l in lines {
        out.push_str(l.as_ref());
        out.push('\n');
    }
    out
}

fn align_to_ancestor<'a>(ancestor: &[&'a str], derived: &[&'a str]) -> (Vec<bool>, Vec<Vec<&'a str>>) {
    let (n, m) = (ancestor.len(), derived.len());
    let width = m + 1;
    let mut dp = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * width + j] = if ancestor[i] == derived[j] {
                dp[(i + 1) * width + j + 1] + 1
            } else {
                dp[(i + 1) * width + j].max(dp[i * width + j + 1])
            };
        }
    }

    let mut keep = vec![false; n];
    let mut insertions = vec![Vec::new(); n + 1];
    let (mut a, mut d) = (0, 0);
    while a < n && d < m {
        if ancestor[a] == derived[d] {
            keep[a] = true;
            a += 1;
            d += 1;
        } else if dp[(a + 1) * width + d] >= dp[a * width + d + 1] {
            a += 1;
        } else {
            insertions[a].push(derived[d]);
            d += 1;
        }
    }
    insertions[n].extend_from_slice(&derived[d..]);

    (keep, insertions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_sides_merge_cleanly() {
        let r = merge("a\nb\n", "a\nb\n", "a\nb\n");
        assert_eq!(r.text, "a\nb\n");
        assert!(!r.has_conflict);
    }

    #[test]
    fn one_side_change_wins() {
        let r = merge("a\nb\nc\n", "a\nB\nc\n", "a\nb\nc\n");
        assert_eq!(r.text, "a\nB\nc\n");
        assert!(!r.has_conflict);
    }

    #[test]
    fn adjacent_insertions_are_not_conflicts() {
        let r = merge("a\nb\nc\n", "a\nX\nb\nc\n", "a\nb\nC\n");
        assert_eq!(r.text, "a\nX\nb\nC\n");
        assert!(!r.has_conflict);
    }

    #[test]
    fn both_sides_appending_keeps_both() {
        let r = merge("a\n", "a\nours\n", "a\ntheirs\n");
        assert_eq!(r.text, "a\nours\ntheirs\n");
        assert!(!r.has_conflict);
    }

    #[test]
    fn same_line_changed_differently_conflicts_and_keeps_both() {
        let r = merge("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        assert!(r.has_conflict);
        assert_eq!(r.text, "a\nX\nY\nc\n");
    }

    #[test]
    fn empty_inputs() {
        let r = merge("", "", "");
        assert_eq!(r.text, "");
        assert!(!r.has_conflict);
        let r = merge("", "new\n", "");
        assert_eq!(r.text, "new\n");
    }

    #[test]
    fn crlf_is_normalized() {
        let r = merge("a\r\nb\r\n", "a\r\nB\r\n", "a\r\nb\r\n");
        assert_eq!(r.text, "a\nB\n");
    }
}
