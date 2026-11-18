use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub author_date: DateTime<FixedOffset>,
    pub message: String,
    pub is_merge: bool,
}

impl CommitInfo {
    pub fn subject(&self) -> &str {
        first_line(&self.message)
    }
}

#[derive(Debug, Clone)]
pub struct Log {
    pub branch: String,
    pub commits: Vec<CommitInfo>,
    pub remote: Option<RemoteStatus>,
}

#[derive(Debug, Clone)]
pub struct RemoteStatus {
    pub branch: String,
    pub sha: Option<String>,
    pub url: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Branches {
    pub current: Option<String>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Rewrite {
    pub new_head: String,
    pub remap: HashMap<String, String>,
    pub backup_ref: String,
    pub auto_resolved_files: Vec<String>,
    pub updated_working_files: Vec<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConflictFile {
    pub path: String,
    pub ours: String,
    pub theirs: String,
    pub unified: String,
    pub with_markers: String,
}

#[derive(Debug, Clone)]
pub struct DiffConflict {
    pub commit_sha: String,
    pub commit_short_sha: String,
    pub commit_subject: String,
    pub files: Vec<ConflictFile>,
}

pub type Resolutions = HashMap<String, HashMap<String, String>>;

pub fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or("")
}
