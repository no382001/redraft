use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use redraft::model::{CommitInfo, DiffConflict, Resolutions, Rewrite};
use redraft::{Error, GitService};
use serde_json::{Value, json};

pub const COMMANDS: &[&str] = &["log", "show", "reword", "edit", "history"];

pub const EXIT_CONFLICT: u8 = 3;

pub const USAGE: &str = "Commands (for scripts and agents; output is JSON unless noted):
  redraft log [-b BRANCH]                    commits on the branch, newest first (editable: on first-parent history)
  redraft show REV                           the commit's own diff (plain text)
  redraft reword REV (-m MSG | -F FILE) [-b BRANCH]
  redraft edit REV --diff FILE [--resolve FILE] [--auto-resolve] [-b BRANCH]
  redraft history                            past rewrites with their backup refs, newest first

  -C PATH     run in this repository instead of the current directory
  REV         anything git understands: a sha, a short sha, HEAD~2, a tag
  FILE        a path, or - for stdin
  BRANCH      defaults to the checked-out branch; REV must be on its first-parent history

  edit takes the output of `show`, changed however you like, as the commit's new diff.
  When replaying a later commit conflicts, edit exits with 3 and prints the conflict:
    {\"status\": \"conflict\", \"commit\": {\"sha\", ...}, \"files\": [{\"path\", \"ours\", \"theirs\",
     \"unified\", \"withMarkers\"}], ...}
  Write the resolved text into the resolve file as {\"<commit sha>\": {\"<path>\": \"<text>\"}}
  and run the same edit again. Keep earlier entries: each run starts over from the
  original commits, so resolutions from every round are needed. The branch is only
  moved once everything replays.

  On success reword and edit print {\"status\": \"ok\", \"newHead\", \"backupRef\", \"remap\", ...}.
  remap maps every old sha to its new one, so later commits can be found again.

Exit codes: 0 ok, 1 error (message on stderr), 2 bad usage, 3 conflict.";

pub fn run(args: Vec<String>) -> ExitCode {
    match dispatch(args) {
        Ok(code) => code,
        Err(Failure::Usage(msg)) => {
            eprintln!("redraft: {msg}\n\nRun `redraft --help` for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Error(Error::Io(e))) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(Failure::Error(e)) => {
            eprintln!("redraft: {e}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Error(e)
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::Error(e.into())
    }
}

type Outcome = std::result::Result<ExitCode, Failure>;

#[derive(Default)]
struct Opts {
    repo: Option<PathBuf>,
    branch: Option<String>,
    message: Option<String>,
    message_file: Option<String>,
    diff: Option<String>,
    resolve: Option<String>,
    auto_resolve: bool,
    positional: Vec<String>,
}

fn parse(args: Vec<String>) -> std::result::Result<(String, Opts), Failure> {
    let mut it = args.into_iter();
    let mut opts = Opts::default();
    let mut command = None;
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| Failure::Usage(format!("{name} needs a value")));
        match arg.as_str() {
            "-C" => opts.repo = Some(PathBuf::from(value("-C")?)),
            "-b" | "--branch" => opts.branch = Some(value(&arg)?),
            "-m" | "--message" => opts.message = Some(value(&arg)?),
            "-F" | "--file" => opts.message_file = Some(value(&arg)?),
            "--diff" => opts.diff = Some(value("--diff")?),
            "--resolve" => opts.resolve = Some(value("--resolve")?),
            "--auto-resolve" => opts.auto_resolve = true,
            "-" => opts.positional.push(arg),
            a if a.starts_with('-') => return Err(Failure::Usage(format!("unknown option {a}"))),
            _ if command.is_none() => command = Some(arg),
            _ => opts.positional.push(arg),
        }
    }
    Ok((command.unwrap_or_default(), opts))
}

fn dispatch(args: Vec<String>) -> Outcome {
    let (command, opts) = parse(args)?;
    let git = GitService::new(GitService::discover(opts.repo.as_deref().unwrap_or(".".as_ref()))?);
    match command.as_str() {
        "log" => log(&git, &opts),
        "show" => show(&git, &opts),
        "reword" => reword(&git, &opts),
        "edit" => edit(&git, &opts),
        "history" => history(&git, &opts),
        other => Err(Failure::Usage(format!("unknown command {other}"))),
    }
}

fn rev(git: &GitService, opts: &Opts) -> std::result::Result<String, Failure> {
    match opts.positional.as_slice() {
        [rev] => Ok(git.resolve(rev)?),
        [] => Err(Failure::Usage("missing REV".into())),
        _ => Err(Failure::Usage(format!("unexpected argument {}", opts.positional[1]))),
    }
}

fn no_positional(opts: &Opts) -> std::result::Result<(), Failure> {
    match opts.positional.first() {
        Some(extra) => Err(Failure::Usage(format!("unexpected argument {extra}"))),
        None => Ok(()),
    }
}

fn read_input(path: &str) -> std::result::Result<String, Failure> {
    if path == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        fs::read_to_string(path).map_err(|e| Failure::Error(Error::User(format!("Can't read {path}: {e}"))))
    }
}

fn print(value: &Value) -> Outcome {
    let mut out = stdout().lock();
    serde_json::to_writer_pretty(&mut out, value).map_err(std::io::Error::from)?;
    writeln!(out)?;
    Ok(ExitCode::SUCCESS)
}

fn log(git: &GitService, opts: &Opts) -> Outcome {
    no_positional(opts)?;
    let log = git.log(opts.branch.as_deref())?;
    let remote = log.remote.map(|r| {
        json!({ "branch": r.branch, "sha": r.sha, "url": r.url, "ahead": r.ahead, "behind": r.behind })
    });
    print(&json!({
        "branch": log.branch,
        "remote": remote,
        "commits": log.commits.iter().map(commit_json).collect::<Vec<_>>(),
    }))
}

fn commit_json(c: &CommitInfo) -> Value {
    json!({
        "sha": c.sha,
        "shortSha": c.short_sha,
        "parents": c.parents,
        "isMerge": c.is_merge,
        "editable": c.editable,
        "author": { "name": c.author_name, "email": c.author_email },
        "date": c.author_date.to_rfc3339(),
        "subject": c.subject(),
        "message": c.message,
    })
}

fn show(git: &GitService, opts: &Opts) -> Outcome {
    let sha = rev(git, opts)?;
    let mut out = stdout().lock();
    out.write_all(git.diff(&sha)?.as_bytes())?;
    Ok(ExitCode::SUCCESS)
}

fn reword(git: &GitService, opts: &Opts) -> Outcome {
    let sha = rev(git, opts)?;
    let message = match (&opts.message, &opts.message_file) {
        (Some(m), None) => m.clone(),
        (None, Some(f)) => read_input(f)?,
        _ => return Err(Failure::Usage("reword needs exactly one of -m MSG or -F FILE".into())),
    };
    if message.trim().is_empty() {
        return Err(Failure::Error(Error::User("The new message is empty.".into())));
    }
    let message = format!("{}\n", message.trim_end());
    print(&rewrite_json(&git.reword(&sha, &message, opts.branch.as_deref())?))
}

fn edit(git: &GitService, opts: &Opts) -> Outcome {
    let sha = rev(git, opts)?;
    let diff = read_input(opts.diff.as_deref().ok_or_else(|| Failure::Usage("edit needs --diff FILE".into()))?)?;
    let resolutions: Resolutions = match &opts.resolve {
        Some(path) => {
            let text = read_input(path)?;
            serde_json::from_str::<HashMap<String, HashMap<String, String>>>(&text).map_err(|e| {
                Failure::Error(Error::User(format!(
                    "{path} isn't a resolve file ({e}); expected {{\"<commit sha>\": {{\"<path>\": \"<text>\"}}}}"
                )))
            })?
        }
        None => HashMap::new(),
    };
    match git.edit_diff(&sha, &diff, opts.branch.as_deref(), &resolutions, opts.auto_resolve) {
        Ok(rewrite) => print(&rewrite_json(&rewrite)),
        Err(Error::Conflict(conflict)) => {
            print(&conflict_json(&conflict))?;
            Ok(ExitCode::from(EXIT_CONFLICT))
        }
        Err(e) => Err(e.into()),
    }
}

fn rewrite_json(r: &Rewrite) -> Value {
    json!({
        "status": "ok",
        "newHead": r.new_head,
        "backupRef": r.backup_ref,
        "remap": r.remap,
        "autoResolvedFiles": r.auto_resolved_files,
        "updatedWorkingFiles": r.updated_working_files,
        "warning": r.warning,
    })
}

fn conflict_json(c: &DiffConflict) -> Value {
    json!({
        "status": "conflict",
        "commit": { "sha": c.commit_sha, "shortSha": c.commit_short_sha, "subject": c.commit_subject },
        "files": c.files.iter().map(|f| json!({
            "path": f.path,
            "ours": f.ours,
            "theirs": f.theirs,
            "unified": f.unified,
            "withMarkers": f.with_markers,
        })).collect::<Vec<_>>(),
        "hint": format!(
            "Add {{\"{}\": {{\"<path>\": \"<resolved text>\"}}}} for every file to the --resolve file, keeping earlier entries, and rerun the same edit.",
            c.commit_sha
        ),
    })
}

fn history(git: &GitService, opts: &Opts) -> Outcome {
    no_positional(opts)?;
    print(&Value::Array(git.audit_log()?.into_iter().map(Value::Object).collect()))
}
