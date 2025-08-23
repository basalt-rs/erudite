use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use derive_more::From;
use leucite::{MemorySize, Rules};

mod builder;
pub use builder::TestContextBuilder;

use crate::{cases::TestCase, runner::TestRunner};

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

        FileConfig {
            src: src.into(),
            dest,
        }
    }

    pub(crate) async fn write_file(&self, base: impl AsRef<Path>) -> std::io::Result<u64> {
        let target = base.as_ref().join(&self.dest);
        match self.src {
            FileContent::Path(ref path) => tokio::fs::copy(path, target).await,
            FileContent::Bytes(ref contents) => tokio::fs::write(target, contents)
                .await
                .map(|_| contents.len() as _),
        }
    }

    pub fn dest(&self) -> &Path {
        &self.dest
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

impl FileContent {
    /// Construct a `FileContent::Path` from something that's like a path
    ///
    /// ```
    /// # use erudite::context::FileContent;
    /// let content = FileContent::path("/foo/bar");
    /// ```
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// Construct a `FileContent::Bytes` from something that's like a string.
    ///
    /// ```
    /// # use erudite::context::FileContent;
    /// let content = FileContent::string("// some rust code");
    /// ```
    pub fn string(string: impl Into<String>) -> Self {
        Self::bytes(string.into())
    }

    /// Construct a `FileContent::Bytes` from raw bytes
    ///
    /// ```
    /// # use erudite::context::FileContent;
    /// let content = FileContent::bytes([0xfa, 0xca, 0xde]);
    /// ```
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Bytes(bytes.into())
    }
}

// TODO: rename (and update test names)
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandConfig<T> {
    None,
    Compile(T),
    Run(T),
    Equal(T),
    Different { compile: T, run: T },
}

impl<T> Default for CommandConfig<T> {
    fn default() -> Self {
        Self::None
    }
}

impl<T> CommandConfig<T> {
    // Can't use From/Into traits because T might be the same as U
    pub fn into<U>(self) -> CommandConfig<U>
    where
        U: From<T>,
    {
        match self {
            CommandConfig::None => CommandConfig::None,
            CommandConfig::Compile(c) => CommandConfig::Compile(c.into()),
            CommandConfig::Run(r) => CommandConfig::Run(r.into()),
            CommandConfig::Equal(t) => CommandConfig::Equal(t.into()),
            CommandConfig::Different { compile, run } => CommandConfig::Different {
                compile: compile.into(),
                run: run.into(),
            },
        }
    }

    pub fn with_compile(&mut self, compile: T) -> &mut Self {
        let old = std::mem::take(self);
        *self = match old {
            Self::None => Self::Compile(compile),
            Self::Compile(_) => Self::Compile(compile),
            Self::Run(run) => Self::Different { compile, run },
            Self::Equal(run) => Self::Different { compile, run },
            Self::Different { compile: _, run } => Self::Different { compile, run },
        };
        self
    }

    pub fn with_run(&mut self, run: T) -> &mut Self {
        let old = std::mem::take(self);
        *self = match old {
            Self::None => Self::Run(run),
            Self::Compile(compile) => Self::Different { compile, run },
            Self::Run(_) => Self::Run(run),
            Self::Equal(compile) => Self::Different { compile, run },
            Self::Different { compile, run: _ } => Self::Different { compile, run },
        };
        self
    }

    pub fn with_both(&mut self, both: T) -> &mut Self {
        *self = Self::Equal(both);
        self
    }

    pub fn compile(&self) -> Option<&T> {
        match self {
            CommandConfig::None => None,
            CommandConfig::Compile(c) => Some(c),
            CommandConfig::Run(_) => None,
            CommandConfig::Equal(c) => Some(c),
            CommandConfig::Different { compile, run: _ } => Some(compile),
        }
    }

    pub fn run(&self) -> Option<&T> {
        match self {
            CommandConfig::None => None,
            CommandConfig::Compile(_) => None,
            CommandConfig::Run(r) => Some(r),
            CommandConfig::Equal(r) => Some(r),
            CommandConfig::Different { compile: _, run } => Some(run),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestContext<T> {
    pub(crate) trim_output: bool,
    pub(crate) files: Vec<FileConfig>,
    pub(crate) test_cases: Vec<TestCase<T>>,
    pub(crate) command: CommandConfig<Box<[String]>>,
    pub(crate) timeout: CommandConfig<Duration>,
    pub(crate) rules: CommandConfig<Rules>,
    pub(crate) max_memory: CommandConfig<MemorySize>,
    pub(crate) max_file_size: CommandConfig<MemorySize>,
    pub(crate) max_threads: CommandConfig<u64>,
}

impl<T> TestContext<T> {
    /// Construct a builder for [`TestContext`], see [`TestContextBuilder`] for more details.
    pub fn builder() -> TestContextBuilder<T> {
        TestContextBuilder::new()
    }

    /// Create a [`TestRunner`] from this context.  See [`TestRunner`] for more details.
    pub fn test_runner<'a>(self: Arc<Self>) -> TestRunner<'a, T> {
        TestRunner::new(self)
    }
}

#[cfg(test)]
mod test {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicI32, Ordering},
    };

    use tmpdir::TmpDir;

    use crate::context::{CommandConfig, FileConfig, FileContent};

    #[tokio::test]
    async fn file_config_path() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();
        let input = tmpdir.as_ref().join("in.rs");
        tokio::fs::write(&input, "some content")
            .await
            .expect("failed setting up test");

        let config = FileConfig::new(input, "out.rs");
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content");
    }

    #[tokio::test]
    async fn file_config_bytes() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();

        let config = FileConfig::new(String::from("some content").into_bytes(), "out.rs");
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content");
    }

    #[test]
    fn file_config_from_tuple2() {
        let cfg: FileConfig = (Path::new("foo/bar"), "foo/bar").into();
        assert_eq!(cfg, FileConfig::new(Path::new("foo/bar"), "foo/bar"));
    }

    #[test]
    fn file_content_path() {
        let content = FileContent::path("foo/bar");
        assert_eq!(content, FileContent::Path(PathBuf::from("foo/bar")));
        let content: FileContent = Path::new("foo/bar").into();
        assert_eq!(content, FileContent::Path(PathBuf::from("foo/bar")));
    }

    #[test]
    fn file_content_string() {
        let content = FileContent::string("hello world");
        assert_eq!(
            content,
            FileContent::Bytes("hello world".as_bytes().to_vec())
        );
    }

    #[test]
    fn file_content_bytes() {
        let bytes = vec![0xca, 0xfe, 0xba, 0xbe];
        let content = FileContent::bytes(bytes.clone());
        assert_eq!(content, FileContent::Bytes(bytes));
    }

    #[test]
    fn commandconfig_run_only() {
        let mut cfg = CommandConfig::default();

        cfg.with_run(42);
        assert_eq!(cfg.run(), Some(&42));
        assert_eq!(cfg.compile(), None);

        cfg.with_run(9001);
        assert_eq!(cfg.run(), Some(&9001));
        assert_eq!(cfg.compile(), None);

        cfg.with_both(1);
        assert_eq!(cfg.run(), Some(&1));
        assert_eq!(cfg.compile(), Some(&1));

        cfg.with_run(42);
        cfg.with_compile(2);
        assert_eq!(cfg.run(), Some(&42));
        assert_eq!(cfg.compile(), Some(&2));
    }

    #[test]
    fn commandconfig_compile_only() {
        let mut cfg = CommandConfig::default();

        cfg.with_compile(42);
        assert_eq!(cfg.run(), None);
        assert_eq!(cfg.compile(), Some(&42));

        cfg.with_compile(9001);
        assert_eq!(cfg.run(), None);
        assert_eq!(cfg.compile(), Some(&9001));

        cfg.with_both(1);
        assert_eq!(cfg.run(), Some(&1));
        assert_eq!(cfg.compile(), Some(&1));

        cfg.with_compile(42);
        cfg.with_run(2);
        assert_eq!(cfg.run(), Some(&2));
        assert_eq!(cfg.compile(), Some(&42));
    }

    #[test]
    fn commandconfig_equal() {
        let mut cfg = CommandConfig::default();

        cfg.with_both(AtomicI32::new(0));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(0));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(0));

        // Change the atomic to ensure that they are both pointing at the same value
        cfg.run().unwrap().store(42, Ordering::SeqCst);
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(42));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(42));

        cfg.with_run(AtomicI32::new(1));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(1));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(42));

        cfg.with_both(AtomicI32::new(69));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(69));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(69));

        cfg.with_compile(AtomicI32::new(8));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(69));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(8));
    }

    #[test]
    fn commandconfig_different() {
        let mut cfg = CommandConfig::default();
        let rval = AtomicI32::new(4);
        let cval = AtomicI32::new(2);
        cfg.with_run(rval).with_compile(cval);

        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(4));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(2));

        // Change the atomic to ensure that they are both pointing at the same value
        cfg.run().unwrap().store(42, Ordering::SeqCst);
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(42));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(2));

        cfg.with_both(AtomicI32::new(1337));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(1337));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(1337));
    }
}
