use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;

use redraft::GitService;

mod cli;
mod tui;

const USAGE: &str = "usage: redraft [repo]
       redraft [-C PATH] COMMAND ...

Browse a branch's history and rewrite it: reword any commit's message or edit
its diff in your editor, with every later commit replayed on top.
repo defaults to the current directory. Press ? inside for keys.

How the replay works:
  Only the first-parent chain from the edited commit up to the branch tip is
  rebuilt. Authors, committers and dates are kept; shas change.
  reword     every later commit keeps its tree as is, so only messages and
             shas change and nothing can conflict.
  edit diff  your diff is applied to the commit's parent, then each later
             commit is cherry-picked onto the new one with a 3-way merge.
             When one conflicts you choose per file: unify, yours, theirs,
             or edit by hand. You can also auto-resolve every file that
             merges cleanly. The new commits are built off to the side. The
             branch moves only once the whole chain has replayed, so
             aborting leaves it untouched.
  Afterwards the old tip is saved under refs/redraft-backup/<branch>-<time>.
  If the branch is checked out, its working files are updated, but only when
  none of the changed paths have uncommitted work.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}\n\n{}", cli::USAGE);
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("redraft {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let first = if args.first().is_some_and(|a| a == "-C") { args.get(2) } else { args.first() };
    if first.is_some_and(|a| cli::COMMANDS.contains(&a.as_str())) {
        return cli::run(args);
    }
    if let Some(bad) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("redraft: unknown option {bad}\n\n{USAGE}");
        return ExitCode::from(2);
    }

    let path = args.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let root = match GitService::discover(&path) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("redraft: {e}");
            return ExitCode::FAILURE;
        }
    };

    if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
        eprintln!("redraft: the browser needs an interactive terminal; see `redraft --help` for commands that don't.");
        return ExitCode::FAILURE;
    }

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    let result = tui::App::new(GitService::new(root)).run(&mut terminal);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("redraft: {e}");
            ExitCode::FAILURE
        }
    }
}
