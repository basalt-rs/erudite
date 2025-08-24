#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use derive_more::From;
// Re-exports so the consumer doesn't need to depend on leucite directly
pub use leucite::{MemorySize, Rules};
use tokio::{io::AsyncRead, task::JoinSet};

pub mod cases;
pub mod context;
pub mod error;
pub mod runner;

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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Output {
    stdout: Bytes,
    stderr: Bytes,
    status: i32,
}

impl Output {
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

// NOTE: For some reason, when using `async`, the returned future is not `Send`.
#[allow(clippy::manual_async_fn)]
fn copy_dir_recursive(
    src: impl Into<PathBuf>,
    dst: impl Into<PathBuf>,
) -> impl std::future::Future<Output = std::io::Result<()>> + Send {
    let src = src.into();
    let dst = dst.into();
    async move {
        let mut js = JoinSet::new();
        tokio::fs::create_dir_all(&dst).await?;
        let mut rd = tokio::fs::read_dir(src).await?;
        while let Some(entry) = rd.next_entry().await? {
            let ty = entry.file_type().await?;
            let dst = dst.join(entry.file_name());
            let src = entry.path();
            if ty.is_dir() {
                js.spawn(copy_dir_recursive(src, dst));
            } else {
                js.spawn(async move { tokio::fs::copy(src, dst).await.map(|_| ()) });
            }
        }

        // join individually, so we fail early
        while let Some(js) = js.join_next().await {
            js??
        }
        Ok(())
    }
}

/// Configuration for how a file should be setup for test cases to be run
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileConfig {
    /// This path is relative to the temporary directory created while running tests
    src: FileContent,
    dest: PathBuf,
}

impl FileConfig {
    pub fn new(src: impl Into<FileContent>, dest: impl AsRef<Path>) -> Self {
        let dest = dest.as_ref();
        let dest = if dest.is_absolute() {
            dest.strip_prefix("/").unwrap().to_path_buf()
        } else {
            dest.to_path_buf()
        };

        Self {
            src: src.into(),
            dest,
        }
    }

    pub(crate) async fn write_file(&self, base: impl AsRef<Path>) -> std::io::Result<()> {
        self.borrow().write_file(base).await
    }

    pub fn dest(&self) -> &Path {
        &self.dest
    }

    pub fn borrow<'a>(&'a self) -> BorrowedFileConfig<'a> {
        BorrowedFileConfig::from_owned(self)
    }
}

impl<S, D> From<(S, D)> for FileConfig
where
    S: Into<FileContent>,
    D: AsRef<Path>,
{
    fn from((source, destination): (S, D)) -> Self {
        Self::new(source, destination)
    }
}

/// Representation of the content of a file to be added into a test environment
///
/// [`FileContent::Path`] represents a path on the host system.  The test runner will copy from
/// this path into the test environment _at compile time_.  If the data should be loaded now,
/// consider using [`FileContent::Bytes`].
///
/// [`FileContent::Bytes`] contains a vec of bytes that will be written to the file when the tests
/// are compiled.
#[derive(Clone, Debug, From, PartialEq, Eq)]
pub enum FileContent {
    /// Copies a file directly from this path
    ///
    /// NOTE: This happens when the tests are compiled/run.  If you want to load the file into
    /// memory first, use [`FileContent::Bytes`].
    Path(PathBuf),
    /// Creates a new file with this content
    Bytes(Vec<u8>),
}

impl From<&Path> for FileContent {
    fn from(value: &Path) -> Self {
        Self::path(value)
    }
}

impl From<&[u8]> for FileContent {
    fn from(value: &[u8]) -> Self {
        Self::bytes(value)
    }
}

impl<const N: usize> From<&[u8; N]> for FileContent {
    fn from(value: &[u8; N]) -> Self {
        Self::bytes(value)
    }
}

impl FileContent {
    /// Construct a `FileContent::Path` from something that's like a path
    ///
    /// ```
    /// # use erudite::FileContent;
    /// let content = FileContent::path("/foo/bar");
    /// ```
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// Construct a `FileContent::Bytes` from something that's like a string.
    ///
    /// ```
    /// # use erudite::FileContent;
    /// let content = FileContent::string("// some rust code");
    /// ```
    pub fn string(string: impl Into<String>) -> Self {
        Self::bytes(string.into())
    }

    /// Construct a `FileContent::Bytes` from raw bytes
    ///
    /// ```
    /// # use erudite::FileContent;
    /// let content = FileContent::bytes([0xfa, 0xca, 0xde]);
    /// ```
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Bytes(bytes.into())
    }
}

#[derive(PartialEq, Debug)]
pub struct BorrowedFileConfig<'a> {
    /// This path is relative to the temporary directory created while running tests
    dest: &'a Path,
    src: BorrowedFileContent<'a>,
}

impl<'a> BorrowedFileConfig<'a> {
    pub fn new(src: impl Into<BorrowedFileContent<'a>>, dest: &'a Path) -> Self {
        let dest = if dest.is_absolute() {
            dest.strip_prefix("/").unwrap()
        } else {
            dest
        };

        Self {
            src: src.into(),
            dest,
        }
    }

    pub fn from_owned(owned: &'a FileConfig) -> Self {
        BorrowedFileConfig {
            dest: owned.dest(),
            src: BorrowedFileContent::from_owned(&owned.src),
        }
    }

    pub fn dest(&self) -> &Path {
        self.dest
    }

    async fn write_file(&mut self, base: impl AsRef<Path>) -> std::io::Result<()> {
        let target = base.as_ref().join(self.dest());
        match self.src.0 {
            BorrowedFileContentInner::Path(ref path) => {
                if tokio::fs::metadata(path).await?.is_dir() {
                    copy_dir_recursive(path.to_path_buf(), target.to_path_buf()).await?;
                } else {
                    tokio::fs::copy(path, target).await?;
                }
            }
            BorrowedFileContentInner::Bytes(ref contents) => {
                tokio::fs::write(target, contents).await?;
            }
            BorrowedFileContentInner::Reader(ref mut reader) => {
                let mut out = tokio::fs::File::create(target).await?;
                tokio::io::copy(reader, &mut out).await?;
            }
        }
        Ok(())
    }
}

impl<'a, S> From<(S, &'a Path)> for BorrowedFileConfig<'a>
where
    S: Into<BorrowedFileContent<'a>>,
{
    fn from((source, destination): (S, &'a Path)) -> Self {
        Self::new(source, destination)
    }
}

/// A trait which is added to all [`AsyncRead`] + [`Unpin`]
#[doc(hidden)]
pub trait AsyncReadUnpin: AsyncRead + Unpin {}
impl<T> AsyncReadUnpin for T where T: AsyncRead + Unpin {}

/// Some form of content that will be used to create a file when a test is compiled
#[derive(From, PartialEq, Debug)]
pub struct BorrowedFileContent<'a>(BorrowedFileContentInner<'a>);
// NOTE: This enum is wrapped so that it can't be created directly by a consuming library
#[derive(From, derive_more::Debug)]
enum BorrowedFileContentInner<'a> {
    /// Copies a file directly from this path
    ///
    /// NOTE: This happens when the tests are actually run.  If you want to load the file into
    /// memory first, use [`Self::Bytes`].
    Path(&'a Path),
    /// Creates a new file with this content
    Bytes(&'a [u8]),
    #[debug("Reader")]
    Reader(&'a mut dyn AsyncReadUnpin),
}

impl PartialEq for BorrowedFileContentInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BorrowedFileContentInner::Path(a), BorrowedFileContentInner::Path(b)) => a == b,
            (BorrowedFileContentInner::Path(_), BorrowedFileContentInner::Bytes(_)) => false,
            (BorrowedFileContentInner::Path(_), BorrowedFileContentInner::Reader(_)) => false,
            (BorrowedFileContentInner::Bytes(_), BorrowedFileContentInner::Path(_)) => false,
            (BorrowedFileContentInner::Bytes(a), BorrowedFileContentInner::Bytes(b)) => a == b,
            (BorrowedFileContentInner::Bytes(_), BorrowedFileContentInner::Reader(_)) => false,
            (BorrowedFileContentInner::Reader(_), BorrowedFileContentInner::Path(_)) => false,
            (BorrowedFileContentInner::Reader(_), BorrowedFileContentInner::Bytes(_)) => false,
            (BorrowedFileContentInner::Reader(_), BorrowedFileContentInner::Reader(_)) => false, // This is not actually possible to check, so assume false
        }
    }
}

impl<'a> BorrowedFileContent<'a> {
    fn from_owned(owned: &'a FileContent) -> Self {
        match owned {
            FileContent::Path(path) => Self::path(path),
            FileContent::Bytes(bytes) => Self::bytes(bytes),
        }
    }

    /// Copy the file directly from this path on the host machine
    ///
    /// Note: The copy happens when tests are compiled.  If you want to load the file into memory
    /// first, use [`TestFileContent::bytes`].
    pub fn path(path: &'a Path) -> Self {
        Self(BorrowedFileContentInner::Path(path))
    }

    /// Write a string to the file
    pub fn string(string: &'a str) -> Self {
        Self(BorrowedFileContentInner::Bytes(string.as_bytes()))
    }

    /// Write bytes to the file
    pub fn bytes(bytes: &'a [u8]) -> Self {
        Self(BorrowedFileContentInner::Bytes(bytes))
    }

    /// Copy from this reader into the file
    pub fn reader<R: AsyncRead + Unpin>(r: &'a mut R) -> Self {
        Self(BorrowedFileContentInner::Reader(r))
    }
}

impl<'a> From<&'a [u8]> for BorrowedFileContent<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self::bytes(value)
    }
}

impl<'a> From<&'a Path> for BorrowedFileContent<'a> {
    fn from(value: &'a Path) -> Self {
        Self::path(value)
    }
}

impl<'a> From<&'a PathBuf> for BorrowedFileContent<'a> {
    fn from(value: &'a PathBuf) -> Self {
        Self::path(value)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn bytes_from_string() {
        let string = "hello".to_string();
        let bytes = Bytes::from(string.clone());
        assert_eq!(bytes, Bytes::String(string.clone()));
        assert_eq!(bytes.bytes(), string.as_bytes());
        assert_eq!(bytes.as_str(), Some(&*string));
        assert_eq!(bytes.to_str_lossy(), string);
        assert_eq!(bytes.len(), string.len());
        assert!(!bytes.is_empty());
    }

    #[test]
    fn bytes_from_vec() {
        const BYTES: &[u8] = &[0xc3, 0x00, b'h', b'i'];
        let bytes = Bytes::from(BYTES.to_vec()); // not a valid string
        assert_eq!(bytes, Bytes::Bytes(BYTES.to_vec()));
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
        let bytes = Bytes::from(BYTES.to_vec());
        assert_eq!(bytes, Bytes::String(STRING.to_string()));
        assert_eq!(bytes.bytes(), BYTES);
        assert_eq!(bytes.as_str(), Some(STRING));
        assert_eq!(bytes.to_str_lossy(), STRING);
        assert_eq!(bytes.len(), STRING.len());
        assert!(!bytes.is_empty());
    }

    #[test]
    fn simple_output_success() {
        let out = Output::new(
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
        let out = Output::new(
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
