use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use leucite::{MemorySize, Rules};

mod builder;
pub use builder::TestContextBuilder;

use crate::{cases::TestCase, runner::TestRunner};

/// Configuration for how a file should be setup for test cases to be run
// TODO: should this be pub?
#[derive(Clone, Debug)]
pub(crate) struct FileConfig {
    /// This path is relative to the temporary directory created while running tests
    dest: PathBuf,
    src: FileContent,
}

impl FileConfig {
    pub(crate) async fn write_file(&self, base: &Path) -> std::io::Result<u64> {
        let target = base.join(&self.dest);
        match self.src {
            FileContent::Path(ref path) => tokio::fs::copy(path, target).await,
            FileContent::Bytes(ref contents) => tokio::fs::write(target, contents)
                .await
                .map(|_| contents.len() as _),
        }
    }

    pub(crate) fn dest(&self) -> &Path {
        &self.dest
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
#[derive(Clone, Debug)]
pub enum FileContent {
    /// Copies a file directly from this path
    ///
    /// NOTE: This happens when the tests are compiled/run.  If you want to load the file into
    /// memory first, use [`FileContent::Bytes`].
    Path(PathBuf),
    /// Creates a new file with this content
    Bytes(Vec<u8>),
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
