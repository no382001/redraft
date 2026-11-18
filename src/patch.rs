use std::sync::LazyLock;

use git2::{Index, IndexEntry, IndexTime, Oid, Repository, Tree};
use regex::Regex;

use crate::error::{Result, user};
use crate::merge::{join_lines, split_lines};

static HUNK_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@@ -(\d+)(?:,\d+)? \+\d+(?:,\d+)? @@").unwrap());

#[derive(Debug, Default, PartialEq)]
pub(crate) struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub is_new: bool,
    pub is_deleted: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Hunk {
    pub old_start: usize,
    pub lines: Vec<(char, String)>,
}

const FILE_MODE: u32 = 0o100644;

pub fn apply(repo: &Repository, base: Option<&Tree>, diff: &str) -> Result<Oid> {
    let files = parse(diff)?;
    let mut index = Index::new()?;
    if let Some(base) = base {
        index.read_tree(base)?;
    }

    for file in files {
        if file.hunks.is_empty() {
            continue;
        }

        if file.is_deleted {
            if let Some(old) = &file.old_path {
                index.remove_path(old.as_ref())?;
            }
            continue;
        }

        let new_path = file.new_path.clone().unwrap_or_default();
        let old_path = file.old_path.clone().unwrap_or_else(|| new_path.clone());
        let (old_content, mode) = if file.is_new {
            (None, FILE_MODE)
        } else {
            let entry = base.and_then(|b| b.get_path(old_path.as_ref()).ok()).ok_or_else(|| {
                crate::error::Error::User(format!("Couldn't apply edited diff: '{old_path}' not found in the parent commit."))
            })?;
            let blob = repo.find_blob(entry.id())?;
            (Some(String::from_utf8_lossy(blob.content()).into_owned()), entry.filemode() as u32)
        };

        let new_content = apply_hunks(old_content.as_deref(), &file.hunks, &new_path)?;

        if file.old_path.as_ref().is_some_and(|old| *old != new_path) {
            index.remove_path(old_path.as_ref())?;
        }
        let mut entry = index_entry(&new_path, mode);
        entry.id = repo.blob(new_content.as_bytes())?;
        index.add(&entry)?;
    }

    Ok(index.write_tree_to(repo)?)
}

pub(crate) fn index_entry(path: &str, mode: u32) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: Oid::ZERO_SHA1,
        flags: path.len().min(0xfff) as u16,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

pub(crate) fn apply_hunks(old: Option<&str>, hunks: &[Hunk], path: &str) -> Result<String> {
    let old_lines = old.map(split_lines).unwrap_or_default();
    let mut result: Vec<&str> = Vec::new();
    let mut cursor = 0;

    for hunk in hunks {
        let start = hunk.old_start.saturating_sub(1);
        if start < cursor || start > old_lines.len() {
            return user(format!("Couldn't apply edited diff to '{path}': a hunk position is out of range."));
        }
        result.extend_from_slice(&old_lines[cursor..start]);
        cursor = start;

        for (kind, text) in &hunk.lines {
            if *kind == '+' {
                result.push(text);
                continue;
            }
            if cursor >= old_lines.len() {
                return user(format!("Couldn't apply edited diff to '{path}': a hunk expects more lines than the file has."));
            }
            if *kind == ' ' {
                result.push(text);
            }
            cursor += 1;
        }
    }
    result.extend_from_slice(&old_lines[cursor..]);

    Ok(join_lines(&result))
}

pub(crate) fn parse(diff: &str) -> Result<Vec<FileDiff>> {
    let normalized = diff.replace("\r\n", "\n");
    let normalized = normalized.strip_suffix('\n').unwrap_or(&normalized);

    let mut files: Vec<FileDiff> = Vec::new();
    let mut in_hunk = false;

    for raw in normalized.split('\n') {
        if raw.starts_with("diff --git ") {
            files.push(FileDiff::default());
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else { continue };

        if let Some(p) = raw.strip_prefix("--- ") {
            file.old_path = parse_path(p);
        } else if let Some(p) = raw.strip_prefix("+++ ") {
            file.new_path = parse_path(p);
        } else if raw.starts_with("new file mode") {
            file.is_new = true;
        } else if raw.starts_with("deleted file mode") {
            file.is_deleted = true;
        } else if raw.starts_with("@@") {
            let caps = HUNK_HEADER
                .captures(raw)
                .ok_or_else(|| crate::error::Error::User(format!("Couldn't parse edited diff: bad hunk header '{raw}'.")))?;
            file.hunks.push(Hunk { old_start: caps[1].parse().unwrap_or(0), lines: Vec::new() });
            in_hunk = true;
        } else if in_hunk {
            let hunk = file.hunks.last_mut().unwrap();
            if raw.starts_with("\\ No newline") {
                continue;
            }
            match raw.chars().next() {
                None => hunk.lines.push((' ', String::new())),
                Some(kind @ (' ' | '+' | '-')) => hunk.lines.push((kind, raw[1..].to_string())),
                _ => {}
            }
        }
    }

    for f in &mut files {
        if f.new_path.is_none() && f.old_path.is_some() {
            f.is_deleted = true;
        }
        if f.old_path.is_none() && f.new_path.is_some() {
            f.is_new = true;
        }
    }
    Ok(files)
}

fn parse_path(p: &str) -> Option<String> {
    let p = p.trim();
    if p == "/dev/null" {
        return None;
    }
    let stripped = if p.len() > 2 && (p.starts_with("a/") || p.starts_with("b/")) { &p[2..] } else { p };
    Some(stripped.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/f.txt b/f.txt\nindex 1..2 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";

    #[test]
    fn parses_a_modification() {
        let files = parse(DIFF).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.old_path.as_deref(), Some("f.txt"));
        assert_eq!(f.new_path.as_deref(), Some("f.txt"));
        assert!(!f.is_new && !f.is_deleted);
        assert_eq!(f.hunks[0].old_start, 1);
        assert_eq!(f.hunks[0].lines.len(), 4);
    }

    #[test]
    fn detects_new_and_deleted_files() {
        let d = "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+x\n\
                 diff --git a/o b/o\ndeleted file mode 100644\n--- a/o\n+++ /dev/null\n@@ -1 +0,0 @@\n-y\n";
        let files = parse(d).unwrap();
        assert!(files[0].is_new && files[0].new_path.as_deref() == Some("n"));
        assert!(files[1].is_deleted && files[1].old_path.as_deref() == Some("o"));
    }

    #[test]
    fn bad_hunk_header_is_a_user_error() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ nonsense @@\n";
        assert!(matches!(parse(d), Err(crate::Error::User(_))));
    }

    #[test]
    fn applies_hunks() {
        let files = parse(DIFF).unwrap();
        let out = apply_hunks(Some("a\nb\nc\n"), &files[0].hunks, "f.txt").unwrap();
        assert_eq!(out, "a\nB\nc\n");
    }

    #[test]
    fn keeps_lines_outside_hunks() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3,1 +3,1 @@\n-3\n+three\n";
        let files = parse(d).unwrap();
        let out = apply_hunks(Some("1\n2\n3\n4\n5\n"), &files[0].hunks, "f").unwrap();
        assert_eq!(out, "1\n2\nthree\n4\n5\n");
    }

    #[test]
    fn hunk_past_end_is_rejected() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -9,1 +9,1 @@\n-x\n+y\n";
        let files = parse(d).unwrap();
        assert!(matches!(apply_hunks(Some("a\n"), &files[0].hunks, "f"), Err(crate::Error::User(_))));
    }

    #[test]
    fn hunk_expecting_more_lines_is_rejected() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n b\n-c\n";
        let files = parse(d).unwrap();
        assert!(matches!(apply_hunks(Some("a\nb\n"), &files[0].hunks, "f"), Err(crate::Error::User(_))));
    }

    #[test]
    fn no_newline_marker_is_ignored_and_blank_lines_are_context() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n\n-c\n+C\n\\ No newline at end of file\n";
        let files = parse(d).unwrap();
        let out = apply_hunks(Some("a\n\nc"), &files[0].hunks, "f").unwrap();
        assert_eq!(out, "a\n\nC\n");
    }
}
