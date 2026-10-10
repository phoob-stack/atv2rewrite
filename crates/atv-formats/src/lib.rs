//! Readers for the data formats used by ATV Quad Power Racing 2 (Xbox).
//!
//! Nothing here ships game data: every reader works on bytes taken from the
//! player's own copy of the game, usually through [`GameSource`].

pub mod anim;
pub mod bark;
pub mod bed;
pub mod bin;
pub mod bog;
pub mod lzss;
pub mod rom;
pub mod row;
pub mod setup;
pub mod source;
pub mod xiso;

pub use source::GameSource;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad magic in {what}: expected {expected:?}")]
    Magic { what: &'static str, expected: &'static str },
    #[error("unexpected end of data while reading {0}")]
    Eof(&'static str),
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, Error>;
