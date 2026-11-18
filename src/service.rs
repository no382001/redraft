use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use git2::build::CheckoutBuilder;
use git2::{
    BranchType, Commit, DiffFormat, ErrorCode, Index, IndexEntry, MergeOptions, Oid, Repository, RepositoryState, Sort,
    StatusOptions, Tree,
};
use serde_json::{Map, Value, json};

use crate::error::{Error, Result, user};
use crate::model::*;
use crate::{merge, patch};

const AUDIT_LOG: &str = "redraft-audit.log";
const BACKUP_PREFIX: &str = "refs/redraft-backup/";

pub struct GitService {
    path: PathBuf,
}

struct RewritePlan<'r> {
    branch_ref: String,
    branch_name: String,
    old_tip: Commit<'r>,
    chain: Vec<Commit<'r>>,
}

impl GitService {
    pub fn discover(path: &Path) -> Result<PathBuf> {
        let full = std::path::absolute(path)?;
        if !full.exists() {
            return user(format!("Path does not exist: {}", full.display()));
        }
        let repo = Repository::discover(&full)
            .map_err(|_| Error::User(format!("Not a git repository: {}", full.display())))?;
        let root = repo.workdir().unwrap_or(repo.path());
        Ok(root.components().collect())
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn open(&self) -> Result<Repository> {
        Ok(Repository::open(&self.path)?)
    }

    pub fn branches(&self) -> Result<Branches> {
        let repo = self.open()?;
        let mut names = Vec::new();
        for b in repo.branches(Some(BranchType::Local))? {
            if let Some(name) = b?.0.name()? {
                names.push(name.to_string());
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        Ok(Branches { current: current_branch(&repo), names })
    }

    pub fn log(&self, branch: Option<&str>) -> Result<Log> {
        let repo = self.open()?;
        let (refname, name) = resolve_branch(&repo, branch)?;
        let tip = repo.refname_to_id(&refname).ok();

        let mut commits = Vec::new();
        if let Some(tip) = tip {
            let mut walk = repo.revwalk()?;
            walk.push(tip)?;
            walk.simplify_first_parent()?;
            walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
            for oid in walk {
                commits.push(commit_info(&repo.find_commit(oid?)?));
            }
        }

        Ok(Log { remote: remote_status(&repo, &name, tip), branch: name, commits })
    }

    pub fn diff(&self, sha: &str) -> Result<String> {
        let repo = self.open()?;
        let commit = find_commit(&repo, sha)?;
        let parent_tree = commit.parents().next().map(|p| p.tree()).transpose()?;
        diff_text(&repo, parent_tree.as_ref(), &commit.tree()?)
    }

    pub fn reword(&self, sha: &str, message: &str, branch: Option<&str>) -> Result<Rewrite> {
        let repo = self.open()?;
        let rw = prepare_rewrite(&repo, sha, branch, "reworded")?;
        let target_sha = rw.chain[0].id().to_string();
        let old_message = rw.chain[0].message_raw().unwrap_or("").to_string();

        let mut remap = HashMap::new();
        let mut previous: Option<Oid> = None;
        for (i, commit) in rw.chain.iter().enumerate() {
            let msg = if i == 0 { message } else { commit.message_raw().unwrap_or("") };
            let new = rebuild(&repo, commit, &commit.tree()?, previous, msg)?;
            remap.insert(commit.id().to_string(), new.to_string());
            previous = Some(new);
        }

        let details = json!({ "targetSha": target_sha, "oldMessage": old_message, "newMessage": message });
        finish_rewrite(&repo, &rw, previous.unwrap(), remap, "reword", details, Vec::new())
    }

    pub fn edit_diff(
        &self,
        sha: &str,
        edited_diff: &str,
        branch: Option<&str>,
        resolutions: &Resolutions,
        auto_resolve: bool,
    ) -> Result<Rewrite> {
        let repo = self.open()?;
        let rw = prepare_rewrite(&repo, sha, branch, "diff-edited")?;
        let target = &rw.chain[0];

        let parent_tree = target.parents().next().map(|p| p.tree()).transpose()?;
        let new_target_tree = repo.find_tree(patch::apply(&repo, parent_tree.as_ref(), edited_diff)?)?;

        check_working_tree(&repo, &rw.branch_ref, &changed_paths(&repo, Some(&target.tree()?), &new_target_tree)?)?;

        let mut remap = HashMap::new();
        let mut auto_resolved = Vec::new();
        let mut previous = rebuild(&repo, target, &new_target_tree, target.parent_ids().next(), target.message_raw().unwrap_or(""))?;
        remap.insert(target.id().to_string(), previous.to_string());

        for commit in &rw.chain[1..] {
            let onto = repo.find_commit(previous)?;
            let mainline = if commit.parent_count() > 1 { 1 } else { 0 };
            let mut index = repo.cherrypick_commit(commit, &onto, mainline, Some(&MergeOptions::new()))?;

            if index.has_conflicts() {
                let given = resolutions.get(&commit.id().to_string());
                let mut unresolved = Vec::new();

                for conflict in index.conflicts()? {
                    let conflict = conflict?;
                    let entry = conflict.ancestor.as_ref().or(conflict.our.as_ref()).or(conflict.their.as_ref()).unwrap();
                    let path = String::from_utf8_lossy(&entry.path).into_owned();
                    let mode = conflict.our.as_ref().or(conflict.their.as_ref()).map_or(0o100644, |e| e.mode);
                    match given.and_then(|g| g.get(&path)) {
                        Some(text) => unresolved.push((path, mode, conflict, Some(text.clone()))),
                        None => unresolved.push((path, mode, conflict, None)),
                    }
                }

                let mut still_conflicted = Vec::new();
                for (path, mode, conflict, resolved) in unresolved {
                    let text_of = |e: &Option<IndexEntry>| -> Result<String> {
                        Ok(match e {
                            Some(e) => String::from_utf8_lossy(repo.find_blob(e.id)?.content()).into_owned(),
                            None => String::new(),
                        })
                    };

                    if let Some(text) = resolved {
                        resolve_in_index(&repo, &mut index, &path, mode, &text)?;
                        continue;
                    }

                    let (ancestor, ours, theirs) = (text_of(&conflict.ancestor)?, text_of(&conflict.our)?, text_of(&conflict.their)?);
                    let merged = merge::merge(&ancestor, &ours, &theirs);

                    let add_add = conflict.ancestor.is_none();
                    let modify_delete = conflict.ancestor.is_some() && (conflict.our.is_none() != conflict.their.is_none());

                    if auto_resolve && !(merged.has_conflict || add_add || modify_delete) {
                        resolve_in_index(&repo, &mut index, &path, mode, &merged.text)?;
                        auto_resolved.push(format!("{}:{path}", short(commit.id())));
                    } else {
                        let with_markers = conflict_markers(&ours, &ancestor, &theirs, commit);
                        still_conflicted.push(ConflictFile { path, ours, theirs, unified: merged.text, with_markers });
                    }
                }

                if !still_conflicted.is_empty() {
                    return Err(Error::Conflict(Box::new(DiffConflict {
                        commit_sha: commit.id().to_string(),
                        commit_short_sha: short(commit.id()),
                        commit_subject: first_line(commit.message_raw().unwrap_or("")).to_string(),
                        files: still_conflicted,
                    })));
                }
            }

            let tree = repo.find_tree(index.write_tree_to(&repo)?)?;
            previous = rebuild(&repo, commit, &tree, Some(previous), commit.message_raw().unwrap_or(""))?;
            remap.insert(commit.id().to_string(), previous.to_string());
        }

        let details = json!({ "targetSha": target.id().to_string(), "autoResolvedFiles": auto_resolved });
        finish_rewrite(&repo, &rw, previous, remap, "diff-edit", details, auto_resolved)
    }

    pub fn audit_log(&self) -> Result<Vec<Map<String, Value>>> {
        let repo = self.open()?;
        let text = match fs::read_to_string(repo.path().join(AUDIT_LOG)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut entries: Vec<_> = text
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str::<Map<String, Value>>(l).ok())
            .collect();
        entries.reverse();
        Ok(entries)
    }
}

fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.find_reference("HEAD").ok()?;
    head.symbolic_target().ok()??.strip_prefix("refs/heads/").map(str::to_string)
}

fn resolve_branch(repo: &Repository, name: Option<&str>) -> Result<(String, String)> {
    match name.filter(|n| !n.is_empty()) {
        Some(n) => {
            let branch = repo
                .find_branch(n, BranchType::Local)
                .or_else(|_| repo.find_branch(n, BranchType::Remote))
                .map_err(|_| Error::User(format!("Branch not found: {n}")))?;
            let refname = branch.get().name()?.to_string();
            Ok((refname, n.to_string()))
        }
        None => match current_branch(repo) {
            Some(b) => Ok((format!("refs/heads/{b}"), b)),
            None => Ok(("HEAD".to_string(), "HEAD".to_string())),
        },
    }
}

fn remote_status(repo: &Repository, branch: &str, tip: Option<Oid>) -> Option<RemoteStatus> {
    let local = repo.find_branch(branch, BranchType::Local).ok();
    let upstream = local
        .and_then(|b| b.upstream().ok())
        .or_else(|| repo.find_branch(&format!("origin/{branch}"), BranchType::Remote).ok())?;

    let refname = upstream.get().name().ok()?.to_string();
    let name = upstream.name().ok()??.to_string();
    let sha = upstream.get().target();
    let url = repo
        .branch_remote_name(&refname)
        .ok()
        .and_then(|r| r.as_str().ok().map(str::to_string))
        .and_then(|r| repo.find_remote(&r).ok()?.url().ok().map(str::to_string));
    let (ahead, behind) = match (tip, sha) {
        (Some(t), Some(s)) => repo.graph_ahead_behind(t, s).map(|(a, b)| (Some(a), Some(b))).unwrap_or((None, None)),
        _ => (None, None),
    };

    Some(RemoteStatus { branch: name, sha: sha.map(|s| s.to_string()), url, ahead, behind })
}

fn commit_info(c: &Commit) -> CommitInfo {
    let author = c.author();
    let when = author.when();
    let offset = FixedOffset::east_opt(when.offset_minutes() * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
    let date: DateTime<FixedOffset> = offset.timestamp_opt(when.seconds(), 0).single().unwrap_or_else(|| Utc::now().into());
    let parents: Vec<String> = c.parent_ids().map(|p| p.to_string()).collect();
    CommitInfo {
        sha: c.id().to_string(),
        short_sha: short(c.id()),
        is_merge: parents.len() > 1,
        parents,
        author_name: String::from_utf8_lossy(author.name_bytes()).into_owned(),
        author_email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
        author_date: date,
        message: String::from_utf8_lossy(c.message_raw_bytes()).into_owned(),
    }
}

fn find_commit<'r>(repo: &'r Repository, sha: &str) -> Result<Commit<'r>> {
    Oid::from_str(sha)
        .ok()
        .and_then(|oid| repo.find_commit(oid).ok())
        .ok_or_else(|| Error::User(format!("Commit not found: {sha}")))
}

fn diff_text(repo: &Repository, old: Option<&Tree>, new: &Tree) -> Result<String> {
    let diff = repo.diff_tree_to_tree(old, Some(new), None)?;
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_, _, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(out)
}

fn short(oid: Oid) -> String {
    oid.to_string()[..7].to_string()
}

fn prepare_rewrite<'r>(repo: &'r Repository, sha: &str, branch: Option<&str>, verb: &str) -> Result<RewritePlan<'r>> {
    if repo.state() != RepositoryState::Clean {
        return user("A git operation (rebase/merge/etc.) is already in progress in this repository.");
    }

    let (branch_ref, branch_name) = resolve_branch(repo, branch)?;
    if !branch_ref.starts_with("refs/heads/") {
        return user(format!(
            "Only local branches can be {verb} (detached HEAD and remote-tracking branches aren't supported)."
        ));
    }
    let old_tip = repo
        .refname_to_id(&branch_ref)
        .ok()
        .and_then(|id| repo.find_commit(id).ok())
        .ok_or_else(|| Error::User("Branch has no commits.".into()))?;
    let target = find_commit(repo, sha)?;

    let mut chain = vec![old_tip.clone()];
    while chain.last().unwrap().id() != target.id() {
        let parent = chain
            .last()
            .unwrap()
            .parents()
            .next()
            .ok_or_else(|| Error::User("Commit is not on this branch's first-parent history.".into()))?;
        chain.push(parent);
    }
    chain.reverse();

    Ok(RewritePlan { branch_ref, branch_name, old_tip, chain })
}

fn rebuild(repo: &Repository, commit: &Commit, tree: &Tree, first_parent: Option<Oid>, message: &str) -> Result<Oid> {
    let mut parents: Vec<Commit> = commit.parents().collect();
    if let (Some(first), Some(slot)) = (first_parent, parents.first_mut()) {
        *slot = repo.find_commit(first)?;
    }
    let parent_refs: Vec<&Commit> = parents.iter().collect();
    Ok(repo.commit(None, &commit.author(), &commit.committer(), message, tree, &parent_refs)?)
}

fn resolve_in_index(repo: &Repository, index: &mut Index, path: &str, mode: u32, text: &str) -> Result<()> {
    index.conflict_remove(Path::new(path))?;
    let mut entry = patch::index_entry(path, mode);
    entry.id = repo.blob(text.as_bytes())?;
    index.add(&entry)?;
    Ok(())
}

fn conflict_markers(ours: &str, ancestor: &str, theirs: &str, theirs_commit: &Commit) -> String {
    let mut s = String::from("<<<<<<< yours (after your edit)\n");
    for (body, sep) in [(ours, "||||||| original\n"), (ancestor, "=======\n"), (theirs, "")] {
        s.push_str(body);
        if !body.is_empty() && !body.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(sep);
    }
    s.push_str(&format!(
        ">>>>>>> {} {}\n",
        short(theirs_commit.id()),
        first_line(theirs_commit.message_raw().unwrap_or(""))
    ));
    s
}

fn finish_rewrite(
    repo: &Repository,
    rw: &RewritePlan,
    new_tip: Oid,
    remap: HashMap<String, String>,
    kind: &str,
    details: Value,
    auto_resolved_files: Vec<String>,
) -> Result<Rewrite> {
    let new_tip_commit = repo.find_commit(new_tip)?;
    let sync_paths = changed_paths(repo, Some(&rw.old_tip.tree()?), &new_tip_commit.tree()?)?;
    check_working_tree(repo, &rw.branch_ref, &sync_paths)?;

    let backup_ref = create_backup_ref(repo, rw, kind)?;

    if let Err(e) = repo.reference_matching(&rw.branch_ref, new_tip, true, rw.old_tip.id(), &format!("redraft: {kind}")) {
        if matches!(e.code(), ErrorCode::Modified | ErrorCode::NotFound) {
            return user(
                "The branch moved while redraft was working. Aborted without changing it; \
                 a backup of the old tip was still recorded, but nothing else happened.",
            );
        }
        return Err(e.into());
    }

    let mut updated_working_files = Vec::new();
    let mut warning = None;
    if is_checked_out(repo, &rw.branch_ref) && !sync_paths.is_empty() {
        match sync_working_tree(repo, &new_tip_commit, &sync_paths) {
            Ok(()) => updated_working_files = sync_paths,
            Err(e) => {
                warning = Some(format!(
                    "History was rewritten, but updating your working files failed ({e}). \
                     Check `git status`; `git checkout HEAD -- <file>` restores a file from the new history."
                ))
            }
        }
    }

    append_audit(repo, kind, &rw.branch_name, new_tip, &backup_ref, details)?;

    Ok(Rewrite {
        new_head: new_tip.to_string(),
        remap,
        backup_ref,
        auto_resolved_files,
        updated_working_files,
        warning,
    })
}

fn create_backup_ref(repo: &Repository, rw: &RewritePlan, kind: &str) -> Result<String> {
    let base = format!("{BACKUP_PREFIX}{}-{}", rw.branch_name.replace(['/', ' '], "-"), Utc::now().timestamp());
    for n in 1.. {
        let name = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        match repo.reference(&name, rw.old_tip.id(), false, &format!("redraft: backup before {kind}")) {
            Ok(_) => return Ok(name),
            Err(e) if e.code() == ErrorCode::Exists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

fn append_audit(repo: &Repository, kind: &str, branch: &str, new_head: Oid, backup_ref: &str, details: Value) -> Result<()> {
    let mut entry = Map::new();
    entry.insert("timestampUtc".into(), Utc::now().to_rfc3339().into());
    entry.insert("kind".into(), kind.into());
    entry.insert("branch".into(), branch.into());
    entry.insert("newHeadSha".into(), new_head.to_string().into());
    entry.insert("backupRef".into(), backup_ref.into());
    if let Value::Object(extra) = details {
        entry.extend(extra);
    }
    let mut line = Value::Object(entry).to_string();
    line.push('\n');
    use std::io::Write;
    fs::OpenOptions::new().create(true).append(true).open(repo.path().join(AUDIT_LOG))?.write_all(line.as_bytes())?;
    Ok(())
}

fn is_checked_out(repo: &Repository, branch_ref: &str) -> bool {
    !repo.is_bare() && current_branch(repo).is_some_and(|b| format!("refs/heads/{b}") == branch_ref)
}

fn changed_paths(repo: &Repository, old: Option<&Tree>, new: &Tree) -> Result<Vec<String>> {
    let diff = repo.diff_tree_to_tree(old, Some(new), None)?;
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            if let Some(p) = file.path().and_then(|p| p.to_str()) {
                if seen.insert(p.to_string()) {
                    paths.push(p.to_string());
                }
            }
        }
    }
    Ok(paths)
}

fn check_working_tree(repo: &Repository, branch_ref: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() || !is_checked_out(repo, branch_ref) {
        return Ok(());
    }

    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true).include_ignored(false).disable_pathspec_match(true);
    for p in paths {
        opts.pathspec(p);
    }

    let dirty: Vec<String> = repo
        .statuses(Some(&mut opts))?
        .iter()
        .filter(|e| !e.status().is_empty() && !e.status().is_ignored())
        .filter_map(|e| e.path().ok().map(str::to_string))
        .collect();

    if dirty.is_empty() {
        return Ok(());
    }
    let mut shown = dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if dirty.len() > 5 {
        shown.push_str(&format!(" and {} more", dirty.len() - 5));
    }
    user(format!("Uncommitted changes in files this rewrite would change: {shown}. Commit or stash them first."))
}

fn sync_working_tree(repo: &Repository, new_tip: &Commit, paths: &[String]) -> Result<()> {
    let tree = new_tip.tree()?;
    let workdir = repo.workdir().ok_or_else(|| Error::User("Repository has no working tree.".into()))?.to_path_buf();
    let (present, removed): (Vec<&String>, Vec<&String>) = paths.iter().partition(|p| tree.get_path(Path::new(p)).is_ok());

    if !removed.is_empty() {
        let mut index = repo.index()?;
        for path in &removed {
            let _ = index.remove_path(Path::new(path));
            let full = workdir.join(path);
            if full.symlink_metadata().is_ok() {
                fs::remove_file(&full)?;
            }
            let mut dir = full.parent();
            while let Some(d) = dir {
                if d == workdir || fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        }
        index.write()?;
    }

    if !present.is_empty() {
        let mut checkout = CheckoutBuilder::new();
        checkout.force().disable_pathspec_match(true).update_index(true);
        for p in &present {
            checkout.path(p.as_str());
        }
        repo.checkout_tree(tree.as_object(), Some(&mut checkout))?;
    }
    Ok(())
}
