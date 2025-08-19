use std::process::Output;

pub mod context;
pub mod runner;

/// Represents some data that may either be a string or a series of bytes.  The recommended method
/// for constructing this type is to use [`From::from`] which will automatically choose the
/// appropriate variant for the data.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum Bytes {
    String(String),
    Bytes(Vec<u8>),
}

impl Bytes {
    pub const fn empty() -> Self {
        Self::Bytes(Vec::new())
    }

    pub fn len(&self) -> usize {
        match self {
            Bytes::String(s) => s.len(),
            Bytes::Bytes(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Bytes::String(s) => s.is_empty(),
            Bytes::Bytes(v) => v.is_empty(),
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Bytes::String(s) => Some(s),
            Bytes::Bytes(_) => None,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Bytes::String(s) => s.as_bytes(),
            Bytes::Bytes(v) => v,
        }
    }
}

impl From<String> for Bytes {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(value: Vec<u8>) -> Self {
        String::from_utf8(value)
            .map(Self::String)
            .map_err(|e| e.into_bytes())
            .unwrap_or_else(Self::Bytes)
    }
}

/// Data which can be returned from a command
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimpleOutput {
    pub stdout: Bytes,
    pub stderr: Bytes,
    pub status: i32,
}

impl Default for SimpleOutput {
    fn default() -> Self {
        Self {
            stdout: Bytes::empty(),
            stderr: Bytes::empty(),
            status: 0,
        }
    }
}

impl From<Output> for SimpleOutput {
    fn from(value: Output) -> Self {
        Self {
            stdout: value.stdout.into(),
            stderr: value.stderr.into(),
            status: value.status.code().unwrap_or(0),
        }
    }
}
