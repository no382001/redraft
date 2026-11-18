use crate::model::DiffConflict;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    User(String),
    #[error("conflict while replaying {}", .0.commit_short_sha)]
    Conflict(Box<DiffConflict>),
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn user<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::User(msg.into()))
}
