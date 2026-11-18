use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;

use gitkit::GitService;

mod tui;

const USAGE: &str = "usage: gitkit [repo]

Browse a branch's history and rewrite it: reword any commit's message or edit
its diff in your editor, with every later commit replayed on top.
repo defaults to the current directory. Press ? inside for keys.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("gitkit {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let Some(bad) = args.iter().find(|a| a.starts_with('-')) {
        eprintln!("gitkit: unknown option {bad}\n\n{USAGE}");
        return ExitCode::from(2);
    }

    let path = args.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let root = match GitService::discover(&path) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("gitkit: {e}");
            return ExitCode::FAILURE;
        }
    };

    if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
        eprintln!("gitkit: needs an interactive terminal.");
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
            eprintln!("gitkit: {e}");
            ExitCode::FAILURE
        }
    }
}
