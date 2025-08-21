use std::borrow::Cow;

pub mod context;
// pub mod old; // TODO: remove me
pub mod runner;

// Re-exports so the consumer doesn't need to depend on leucite directly
pub use leucite::{MemorySize, Rules};

/// Represents some data that may either be a string or a series of bytes.  The recommended method
/// for constructing this type is to use [`From::from`] which will automatically choose the
/// appropriate variant for the data.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Bytes {
    String(String),
    Bytes(Vec<u8>),
}

impl Bytes {
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

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Bytes::String(s) => Some(s),
            Bytes::Bytes(_) => None,
        }
    }

    pub fn to_str_lossy(&self) -> Cow<'_, str> {
        match self {
            Bytes::String(ref s) => Cow::Borrowed(s),
            Bytes::Bytes(ref bytes) => String::from_utf8_lossy(bytes),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Bytes::String(s) => s.as_bytes(),
            Bytes::Bytes(v) => v,
        }
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SimpleOutput {
    stdout: Bytes,
    stderr: Bytes,
    status: i32,
}

impl SimpleOutput {
    pub(crate) fn new(stdout: impl Into<Bytes>, stderr: impl Into<Bytes>, status: i32) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status,
        }
    }

    pub fn stdout(&self) -> &Bytes {
        &self.stdout
    }

    pub fn stderr(&self) -> &Bytes {
        &self.stderr
    }

    pub fn exit_status(&self) -> i32 {
        self.status
    }

    pub fn success(&self) -> bool {
        self.status == 0
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn bytes_from_vec() {
        const BYTES: &[u8] = &[0xc3, 0x00, b'h', b'i'];
        let bytes = Bytes::from(BYTES.to_vec()); // not a valid string
        assert!(matches!(bytes, Bytes::Bytes(_)));
        assert_eq!(bytes.bytes(), BYTES);
        assert!(bytes.as_str().is_none());
        assert_eq!(bytes.to_str_lossy(), "�\0hi");
        assert_eq!(bytes.len(), 4);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn bytes_from_string_vec() {
        const STRING: &str = "hello";
        const BYTES: &[u8] = STRING.as_bytes();
        let bytes = Bytes::from(BYTES.to_vec()); // not a valid string
        assert!(matches!(bytes, Bytes::String(_)));
        assert_eq!(bytes.bytes(), BYTES);
        assert_eq!(bytes.as_str(), Some(STRING));
        assert_eq!(bytes.to_str_lossy(), STRING);
        assert_eq!(bytes.len(), STRING.len());
        assert!(!bytes.is_empty());
    }

    #[test]
    fn simple_output_success() {
        let out = SimpleOutput::new(
            "hello".to_string().into_bytes(),
            "world".to_string().into_bytes(),
            0,
        );

        assert_eq!(out.stdout().as_str(), Some("hello"));
        assert_eq!(out.stderr().as_str(), Some("world"));
        assert_eq!(out.exit_status(), 0);
        assert!(out.success());
    }

    #[test]
    fn simple_output_fail() {
        let out = SimpleOutput::new(
            "hello".to_string().into_bytes(),
            "world".to_string().into_bytes(),
            1,
        );

        assert_eq!(out.stdout().as_str(), Some("hello"));
        assert_eq!(out.stderr().as_str(), Some("world"));
        assert_eq!(out.exit_status(), 1);
        assert!(!out.success());
    }
}
