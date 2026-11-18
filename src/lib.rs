pub mod error;
pub mod merge;
pub mod model;
pub mod patch;

pub use error::{Error, Result};
pub mod service;

pub use service::GitService;
