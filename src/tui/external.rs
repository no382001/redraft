use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};

pub struct External {
    repo: PathBuf,
    temp_dir: PathBuf,
}

impl External {
    pub fn new(repo: &Path) -> Self {
        Self { repo: repo.to_path_buf(), temp_dir: std::env::temp_dir().join(format!("redraft-{}", std::process::id())) }
    }

    pub fn edit(&self, term: &mut DefaultTerminal, text: &str, file_name: &str) -> Option<String> {
        fs::create_dir_all(&self.temp_dir).ok()?;
        let path = self.temp_dir.join(file_name);
        fs::write(&path, text).ok()?;

        let editor = self.editor();
        let ok = suspend(term, || {
            Command::new("sh")
                .args(["-c", &format!("{editor} \"$1\""), &editor])
                .arg(&path)
                .status()
                .is_ok_and(|s| s.success())
        });

        let result = if ok { fs::read_to_string(&path).ok() } else { None };
        let _ = fs::remove_file(&path);
        result
    }

    pub fn show(&self, term: &mut DefaultTerminal, sha: &str) {
        suspend(term, || {
            let _ = Command::new("git").arg("-C").arg(&self.repo).args(["show", "--stat", "--patch", sha]).status();
        });
    }

    fn editor(&self) -> String {
        let from_git = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["var", "GIT_EDITOR"])
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        from_git
            .or_else(|| std::env::var("VISUAL").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "vi".to_string())
    }
}

impl Drop for External {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

pub fn suspend<T>(term: &mut DefaultTerminal, f: impl FnOnce() -> T) -> T {
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let result = f();
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen, EnableMouseCapture);
    let _ = term.clear();
    result
}
