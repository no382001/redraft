use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use redraft::model::Resolutions;
use redraft::{Error, GitService};

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
        r.write("sub/x.txt", "x\n");
        r.write("a*b.txt", "g\n");
        r.sh("git add -A && git commit -qm one");
        r.write("f.txt", "a\nb\nc\nd\n");
        r.sh("git commit -qam two");
        r.write("other.txt", "u\n");
        r.sh("git add other.txt && git commit -qm three");
        r
    }

    fn sh(&self, cmd: &str) -> String {
        let out = Command::new("sh").args(["-c", cmd]).current_dir(&self.root).output().unwrap();
        assert!(out.status.success(), "`{cmd}` failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, path: &str, text: &str) {
        let p = self.root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap()
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }

    fn rev(&self, spec: &str) -> String {
        self.sh(&format!("git rev-parse {spec}"))
    }

    fn status(&self) -> String {
        self.sh("git status --porcelain")
    }

    fn svc(&self) -> GitService {
        GitService::new(&self.root)
    }

    fn edited_root_diff(&self, from: &str, to: &str) -> (String, String) {
        let one = self.rev("main~2");
        let diff = self.svc().diff(&one).unwrap();
        assert!(diff.contains(from), "diff doesn't contain {from:?}:\n{diff}");
        (one, diff.replace(from, to))
    }
}

fn no_resolutions() -> Resolutions {
    HashMap::new()
}

fn user_error(r: Result<impl std::fmt::Debug, Error>) -> String {
    match r {
        Err(Error::User(msg)) => msg,
        other => panic!("expected a user error, got {other:?}"),
    }
}

#[test]
fn discover_finds_root_from_subdirectory() {
    let r = Repo::new();
    let found = GitService::discover(&r.root.join("sub")).unwrap();
    assert_eq!(found.canonicalize().unwrap(), r.root.canonicalize().unwrap());
}

#[test]
fn discover_rejects_non_repo_and_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    assert!(user_error(GitService::discover(dir.path())).contains("Not a git repository"));
    assert!(user_error(GitService::discover(&dir.path().join("nope"))).contains("does not exist"));
}

#[test]
fn branches_and_log() {
    let r = Repo::new();
    r.sh("git branch feature");
    let b = r.svc().branches().unwrap();
    assert_eq!(b.current.as_deref(), Some("main"));
    assert_eq!(b.names, vec!["feature", "main"]);

    let log = r.svc().log(None).unwrap();
    assert_eq!(log.branch, "main");
    let subjects: Vec<_> = log.commits.iter().map(|c| c.subject().to_string()).collect();
    assert_eq!(subjects, ["three", "two", "one"]);
    assert_eq!(log.commits[0].author_name, "T");
    assert_eq!(log.commits[0].short_sha.len(), 7);
    assert!(log.remote.is_none());
}

#[test]
fn log_includes_merged_commits_and_flags_editable() {
    let r = Repo::new();
    r.sh("git checkout -q -b side main~1 && echo s > s.txt && git add s.txt && git commit -qm side && git checkout -q main && git merge -q --no-ff -m merged side");
    let log = r.svc().log(None).unwrap();
    let mut subjects: Vec<_> = log.commits.iter().map(|c| c.subject().to_string()).collect();
    assert_eq!(subjects.remove(0), "merged");
    subjects.sort();
    assert_eq!(subjects, ["one", "side", "three", "two"]);
    assert!(log.commits[0].is_merge);
    for c in &log.commits {
        assert_eq!(c.editable, c.subject() != "side", "{}", c.subject());
    }
    assert!(r.svc().reword(&r.rev("side"), "x", None).is_err());
}

#[test]
fn log_reports_remote_ahead_behind() {
    let r = Repo::new();
    r.sh("git init -q --bare ../remote.git && git remote add origin ../remote.git && git push -q origin main 2>/dev/null");
    r.write("f.txt", "local\n");
    r.sh("git commit -qam four");
    let log = r.svc().log(None).unwrap();
    let remote = log.remote.unwrap();
    assert_eq!(remote.branch, "origin/main");
    assert_eq!((remote.ahead, remote.behind), (Some(1), Some(0)));
    assert_eq!(remote.sha.as_deref(), Some(r.rev("main~1").as_str()));
    assert!(remote.url.unwrap().ends_with("remote.git"));
}

#[test]
fn diff_matches_git_show() {
    let r = Repo::new();
    let two = r.rev("main~1");
    let ours = r.svc().diff(&two).unwrap();
    assert!(ours.contains("diff --git a/f.txt b/f.txt"));
    assert!(ours.contains("@@ -1,3 +1,4 @@\n a\n b\n c\n+d\n"));
}

#[test]
fn unknown_commit_is_a_user_error() {
    let r = Repo::new();
    assert!(user_error(r.svc().diff("deadbeef")).contains("Commit not found"));
}

#[test]
fn reword_rebuilds_descendants_and_records_backup_and_audit() {
    let r = Repo::new();
    let (old_tip, one) = (r.rev("main"), r.rev("main~2"));
    let result = r.svc().reword(&one, "one (renamed)\n", None).unwrap();

    assert_eq!(r.sh("git log --format=%s"), "three\ntwo\none (renamed)");
    assert_eq!(result.remap.len(), 3);
    assert_eq!(result.remap[&one], r.rev("main~2"));
    assert_eq!(r.rev(&result.backup_ref), old_tip);
    assert_eq!(r.rev("main^{tree}"), r.rev(&format!("{old_tip}^{{tree}}")), "trees must not change");
    assert!(result.updated_working_files.is_empty());

    let audit = r.svc().audit_log().unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["kind"], "reword");
    assert_eq!(audit[0]["oldMessage"], "one\n");
    assert_eq!(audit[0]["newMessage"], "one (renamed)\n");
    assert_eq!(audit[0]["backupRef"], result.backup_ref.as_str());
}

#[test]
fn reword_keeps_author_and_committer() {
    let r = Repo::new();
    r.sh("GIT_AUTHOR_DATE='2001-02-03T04:05:06+01:00' GIT_COMMITTER_NAME=C git commit -q --amend --no-edit --author='Someone <s@x>'");
    let before = r.sh("git log -1 --format='%an|%ae|%ad|%cn'");
    r.svc().reword(&r.rev("main"), "new\n", None).unwrap();
    assert_eq!(r.sh("git log -1 --format='%an|%ae|%ad|%cn'"), before);
}

#[test]
fn reword_with_dirty_tree_is_fine() {
    let r = Repo::new();
    r.write("f.txt", "local\n");
    r.svc().reword(&r.rev("main~2"), "renamed\n", None).unwrap();
    assert_eq!(r.read("f.txt"), "local\n");
}

#[test]
fn rewrite_refuses_commits_off_the_branch_and_in_progress_operations() {
    let r = Repo::new();
    r.sh("git checkout -q -b side main~1 && echo s > s.txt && git add s.txt && git commit -qm side && git checkout -q main");
    let side = r.rev("side");
    assert!(user_error(r.svc().reword(&side, "x\n", None)).contains("first-parent"));

    fs::write(r.root.join(".git/MERGE_HEAD"), r.rev("side")).unwrap();
    assert!(user_error(r.svc().reword(&r.rev("main"), "x\n", None)).contains("in progress"));
}

#[test]
fn rewrite_refuses_detached_head() {
    let r = Repo::new();
    r.sh("git checkout -q --detach");
    assert!(user_error(r.svc().reword(&r.rev("HEAD"), "x\n", None)).contains("Only local branches"));
}

#[test]
fn diff_edit_updates_clean_working_tree() {
    let r = Repo::new();
    let (one, diff) = r.edited_root_diff("+b\n", "+B\n");
    let result = r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false).unwrap();
    assert_eq!(r.read("f.txt"), "a\nB\nc\nd\n");
    assert_eq!(r.status(), "");
    assert_eq!(result.updated_working_files, ["f.txt"]);
    assert_eq!(r.sh("git show main~2:f.txt"), "a\nB\nc");
}

#[test]
fn diff_edit_deleting_files_removes_them_and_empty_folders() {
    let r = Repo::new();
    let one = r.rev("main~2");
    let diff = r.svc().diff(&one).unwrap();
    let kept: String = diff.split_inclusive('\n').fold((String::new(), false), |(mut acc, mut keep), line| {
        if line.starts_with("diff --git ") {
            keep = line.contains("f.txt");
        }
        if keep {
            acc.push_str(line);
        }
        (acc, keep)
    }).0;

    r.svc().edit_diff(&one, &kept, None, &no_resolutions(), false).unwrap();
    assert!(!r.exists("sub/x.txt"));
    assert!(!r.exists("sub"));
    assert!(!r.exists("a*b.txt"));
    assert_eq!(r.status(), "");
}

#[test]
fn diff_edit_adding_a_file_creates_it_with_mode() {
    let r = Repo::new();
    let one = r.rev("main~2");
    let add = "diff --git a/deep/new.sh b/deep/new.sh\nnew file mode 100755\n--- /dev/null\n+++ b/deep/new.sh\n@@ -0,0 +1 @@\n+hi\n";
    let diff = r.svc().diff(&one).unwrap() + add;
    r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false).unwrap();
    assert_eq!(r.read("deep/new.sh"), "hi\n");
    assert_eq!(r.status(), "");
}

#[test]
fn diff_edit_leaves_unrelated_local_changes_alone() {
    let r = Repo::new();
    r.write("other.txt", "local work\n");
    let (one, diff) = r.edited_root_diff("+b\n", "+B\n");
    r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false).unwrap();
    assert_eq!(r.read("other.txt"), "local work\n");
    assert_eq!(r.status(), "M other.txt");
}

#[test]
fn diff_edit_refuses_overlapping_local_changes_before_touching_anything() {
    let r = Repo::new();
    let head = r.rev("main");
    r.write("f.txt", "local\n");
    let (one, diff) = r.edited_root_diff("+b\n", "+B\n");
    assert!(user_error(r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false)).contains("f.txt"));
    assert_eq!(r.rev("main"), head);
    assert_eq!(r.sh("git for-each-ref refs/redraft-backup"), "");
    assert_eq!(r.read("f.txt"), "local\n");
}

#[test]
fn diff_edit_refuses_staged_changes_in_overlapping_files() {
    let r = Repo::new();
    r.sh("echo staged >> f.txt && git add f.txt && git checkout -q -- f.txt");
    let (one, diff) = r.edited_root_diff("+b\n", "+B\n");
    assert!(user_error(r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false)).contains("f.txt"));
}

#[test]
fn diff_edit_refuses_to_overwrite_untracked_file() {
    let r = Repo::new();
    r.write("new.txt", "mine\n");
    let one = r.rev("main~2");
    let add = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+theirs\n";
    let diff = r.svc().diff(&one).unwrap() + add;
    assert!(user_error(r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false)).contains("new.txt"));
    assert_eq!(r.read("new.txt"), "mine\n");
}

#[test]
fn glob_like_filename_is_not_treated_as_a_pattern() {
    let r = Repo::new();
    r.write("aXb.txt", "y\n");
    r.sh("git add aXb.txt && git commit -qm four");
    r.write("aXb.txt", "local\n");
    let one = r.rev("main~3");
    let diff = r.svc().diff(&one).unwrap().replace("+g\n", "+G\n");
    r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false).unwrap();
    assert_eq!(r.read("a*b.txt"), "G\n");
    assert_eq!(r.read("aXb.txt"), "local\n");
}

#[test]
fn diff_edit_on_other_branch_leaves_working_tree_alone() {
    let r = Repo::new();
    r.sh("git branch side && git checkout -q -b elsewhere main~1");
    r.write("f.txt", "local\n");
    let one = r.rev("side~2");
    let diff = r.svc().diff(&one).unwrap().replace("+b\n", "+B\n");
    let result = r.svc().edit_diff(&one, &diff, Some("side"), &no_resolutions(), false).unwrap();
    assert_eq!(r.sh("git show side~2:f.txt"), "a\nB\nc");
    assert_eq!(r.read("f.txt"), "local\n");
    assert!(result.updated_working_files.is_empty());
}

#[test]
fn bad_edited_diff_is_a_user_error() {
    let r = Repo::new();
    let one = r.rev("main~2");
    let diff = r.svc().diff(&one).unwrap().replacen("@@ -0,0 +1,3 @@", "@@ garbage @@", 1);
    assert!(user_error(r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false)).contains("hunk header"));
}

#[test]
fn conflict_round_trip_with_resolutions() {
    let r = Repo::new();
    let (one, diff) = r.edited_root_diff("+c\n", "+C\n");
    let head = r.rev("main");

    let conflict = match r.svc().edit_diff(&one, &diff, None, &no_resolutions(), false) {
        Err(Error::Conflict(c)) => c,
        other => panic!("expected a conflict, got {other:?}"),
    };
    assert_eq!(r.rev("main"), head, "nothing may change until the chain replays");
    assert_eq!(conflict.commit_subject, "two");
    assert_eq!(conflict.files.len(), 1);
    let file = &conflict.files[0];
    assert_eq!(file.path, "f.txt");
    assert_eq!(file.ours, "a\nb\nC\n");
    assert_eq!(file.theirs, "a\nb\nc\nd\n");
    assert!(file.with_markers.starts_with("<<<<<<< yours (after your edit)\na\nb\nC\n||||||| original\n"));
    assert!(file.with_markers.contains(&format!(">>>>>>> {} two\n", conflict.commit_short_sha)));

    let mut resolutions = no_resolutions();
    resolutions.entry(conflict.commit_sha.clone()).or_default().insert("f.txt".into(), "a\nb\nC\nd\n".into());
    r.svc().edit_diff(&one, &diff, None, &resolutions, false).unwrap();

    assert_eq!(r.sh("git show main~2:f.txt"), "a\nb\nC");
    assert_eq!(r.sh("git show main~1:f.txt"), "a\nb\nC\nd");
    assert_eq!(r.read("f.txt"), "a\nb\nC\nd\n");
    assert_eq!(r.status(), "");
}

#[test]
fn auto_resolve_merges_clean_files_and_reports_them() {
    let r = Repo::new();
    let (one, diff) = r.edited_root_diff("@@ -0,0 +1,3 @@\n+a\n+b\n+c\n", "@@ -0,0 +1,4 @@\n+top\n+a\n+b\n+c\n");
    let result = r.svc().edit_diff(&one, &diff, None, &no_resolutions(), true).unwrap();
    assert_eq!(r.sh("git show main:f.txt"), "top\na\nb\nc\nd");
    assert_eq!(r.status(), "");
    let audit = r.svc().audit_log().unwrap();
    assert_eq!(audit[0]["kind"], "diff-edit");
    assert_eq!(audit[0]["autoResolvedFiles"].as_array().unwrap().len(), result.auto_resolved_files.len());
}

#[test]
fn auto_resolve_still_asks_about_real_conflicts() {
    let r = Repo::new();
    r.write("f.txt", "a\nb\nX\nd\n");
    r.sh("git commit -qam four");
    let one = r.rev("main~3");
    let diff = r.svc().diff(&one).unwrap().replace("+c\n", "+C\n");
    match r.svc().edit_diff(&one, &diff, None, &no_resolutions(), true) {
        Err(Error::Conflict(c)) => assert_eq!(c.files[0].path, "f.txt"),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn audit_log_is_newest_first() {
    let r = Repo::new();
    r.svc().reword(&r.rev("main~2"), "first\n", None).unwrap();
    r.svc().reword(&r.rev("main~2"), "second\n", None).unwrap();
    let audit = r.svc().audit_log().unwrap();
    assert_eq!(audit[0]["newMessage"], "second\n");
    assert_eq!(audit[1]["newMessage"], "first\n");
}
