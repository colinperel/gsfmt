// ───────────────────────────────────────────────────────────── errors ──

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    /// Character offset into the source formula.
    pub pos: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at character {})", self.msg, self.pos + 1)
    }
}

impl std::error::Error for Error {}

pub(crate) fn err<T>(msg: impl Into<String>, pos: usize) -> Result<T, Error> {
    Err(Error {
        msg: msg.into(),
        pos,
    })
}
