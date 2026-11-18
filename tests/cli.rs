use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::io::Write;

use serde_json::Value;

struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        let r = Repo { _dir: dir, root };
        r.sh("git init -q -b main && git config user.name T && git config user.email t@e.c && git config commit.gpgsign false");
        r.write("f.txt", "a\nb\nc\n");
        r.sh("git add -A && git commit -qm one");
        r.write("f.txt", "a\nb\nc\nd\n");
        r.sh("git commit -qam two");
        r.write("f.txt", "a\nB\nc\nd\n");
        r.sh("git commit -qam three");
        r
    }

    fn sh(&self, cmd: &str) -> String {
        let out = Command::new("sh").args(["-c", cmd]).current_dir(&self.root).output().unwrap();
        assert!(out.status.success(), "`{cmd}` failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.root.join(path), text).unwrap();
    }

    fn path(&self, name: &str) -> String {
        self._dir.path().join(name).to_string_lossy().into_owned()
    }

    fn redraft(&self, args: &[&str]) -> Output {
        self.redraft_stdin(args, "")
    }

    fn redraft_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_redraft"))
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

#[test]
fn log_lists_commits_newest_first() {
    let r = Repo::new();
    let out = r.redraft(&["log"]);
    assert_eq!(code(&out), 0);
    let v = json(&out);
    assert_eq!(v["branch"], "main");
    let subjects: Vec<_> = v["commits"].as_array().unwrap().iter().map(|c| c["subject"].as_str().unwrap()).collect();
    assert_eq!(subjects, ["three", "two", "one"]);
    assert_eq!(v["commits"][0]["sha"], r.sh("git rev-parse HEAD"));
}

#[test]
fn dash_c_runs_in_another_repo() {
    let r = Repo::new();
    let out = Command::new(env!("CARGO_BIN_EXE_redraft")).args(["-C", r.root.to_str().unwrap(), "log"]).current_dir(Path::new("/")).output().unwrap();
    assert_eq!(code(&out), 0);
    assert_eq!(json(&out)["commits"].as_array().unwrap().len(), 3);
}

#[test]
fn show_prints_the_commits_own_diff() {
    let r = Repo::new();
    let out = r.redraft(&["show", "HEAD~1"]);
    assert_eq!(code(&out), 0);
    let diff = String::from_utf8(out.stdout).unwrap();
    assert!(diff.contains("+d\n"), "{diff}");
    assert!(!diff.contains("+a\n"), "{diff}");
}

#[test]
fn reword_from_stdin_and_reports_remap() {
    let r = Repo::new();
    let old_two = r.sh("git rev-parse HEAD~1");
    let out = r.redraft_stdin(&["reword", "HEAD~1", "-F", "-"], "two, better\n\nbody\n\n\n");
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let v = json(&out);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["newHead"], r.sh("git rev-parse HEAD"));
    assert_eq!(v["remap"][&old_two], r.sh("git rev-parse HEAD~1"));
    assert_eq!(r.sh("git log -1 --format=%B HEAD~1"), "two, better\n\nbody");
    assert!(v["backupRef"].as_str().unwrap().starts_with("refs/redraft-backup/main-"));
}

#[test]
fn edit_conflict_then_resolve_file_finishes() {
    let r = Repo::new();
    let diff = String::from_utf8(r.redraft(&["show", "HEAD~2"]).stdout).unwrap().replace("+b\n", "+BB\n");
    let diff_path = r.path("edit.diff");
    fs::write(&diff_path, &diff).unwrap();
    let head = r.sh("git rev-parse HEAD");

    let out = r.redraft(&["edit", "HEAD~2", "--diff", &diff_path]);
    assert_eq!(code(&out), 3);
    let v = json(&out);
    assert_eq!(v["status"], "conflict");
    assert_eq!(v["commit"]["sha"], head);
    assert_eq!(v["files"][0]["path"], "f.txt");
    assert_eq!(r.sh("git rev-parse HEAD"), head, "branch must not move on conflict");

    let resolve_path = r.path("resolve.json");
    fs::write(&resolve_path, serde_json::json!({ head.clone(): { "f.txt": "a\nBB\nc\nd\n" } }).to_string()).unwrap();
    let out = r.redraft(&["edit", "HEAD~2", "--diff", &diff_path, "--resolve", &resolve_path]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(json(&out)["status"], "ok");
    assert_eq!(r.sh("git show HEAD:f.txt"), "a\nBB\nc\nd");
    assert_eq!(r.sh("git show HEAD~2:f.txt"), "a\nBB\nc");
}

#[test]
fn history_lists_rewrites() {
    let r = Repo::new();
    assert_eq!(code(&r.redraft(&["reword", "HEAD", "-m", "three!"])), 0);
    let v = json(&r.redraft(&["history"]));
    assert_eq!(v[0]["kind"], "reword");
}

#[test]
fn usage_and_user_errors_have_distinct_exit_codes() {
    let r = Repo::new();
    assert_eq!(code(&r.redraft(&["reword", "HEAD"])), 2);
    assert_eq!(code(&r.redraft(&["log", "--nope"])), 2);
    let out = r.redraft(&["show", "nosuchrev"]);
    assert_eq!(code(&out), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Commit not found"));
    assert_eq!(code(&r.redraft(&["reword", "HEAD", "-m", "  "])), 1);
}
