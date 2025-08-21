use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use derive_more::From;
use leucite::{MemorySize, Rules};

mod builder;
pub use builder::TestContextBuilder;

use crate::runner::TestRunner;

#[cfg(all(feature = "serde", feature = "regex"))]
mod regex_serde {
    use std::borrow::Cow;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &regex::Regex, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_str().serialize(serializer)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<regex::Regex, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <Cow<str>>::deserialize(d)?;

        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, From)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExpectedOutput {
    String(#[from] String),
    #[cfg(feature = "regex")]
    Regex(
        #[from]
        #[serde(with = "regex_serde")]
        regex::Regex,
    ),
}

impl From<&str> for ExpectedOutput {
    fn from(value: &str) -> Self {
        ExpectedOutput::String(value.into())
    }
}

// TODO: output validator trait?
#[derive(Debug, Clone, From)]
pub struct OutputValidator {
    pub(crate) trim_output: bool,
    pub(crate) expected_output: ExpectedOutput,
}

impl OutputValidator {
    pub(crate) fn is_valid(&self, output: impl AsRef<str>) -> bool {
        let output = output.as_ref();
        let output = if self.trim_output {
            output.trim()
        } else {
            output
        };

        match self.expected_output {
            ExpectedOutput::String(ref s) => s == output,
            #[cfg(feature = "regex")]
            ExpectedOutput::Regex(ref reg) => reg.is_match(output),
        }
    }
}

/// A test case which contains input, output, and some associated data
#[derive(Debug, Clone)]
pub struct TestCase<T = ()> {
    pub(crate) input: String,
    pub(crate) output: ExpectedOutput,
    pub(crate) data: T,
}

impl<T> TestCase<T> {
    pub fn new(input: impl Into<String>, output: impl Into<ExpectedOutput>, data: T) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            data,
        }
    }

    /// Retrieve the input value associated with this test case
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Retrieve the expected output for this test case
    pub fn output(&self) -> &ExpectedOutput {
        &self.output
    }

    /// Get the data assocated with this test case
    ///
    /// See also: [`TestCase::into_data`]
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Get the owned data assocated with this test case
    ///
    /// See also: [`TestCase::data`]
    pub fn into_data(self) -> T {
        self.data
    }
}

impl<I, O, T> From<(I, O)> for TestCase<T>
where
    I: Into<String>,
    O: Into<ExpectedOutput>,
    T: Default,
{
    fn from((input, output): (I, O)) -> Self {
        Self::new(input, output, T::default())
    }
}

impl<I, O, T> From<(I, O, T)> for TestCase<T>
where
    I: Into<String>,
    O: Into<ExpectedOutput>,
{
    fn from((input, output, data): (I, O, T)) -> Self {
        Self::new(input, output, data)
    }
}

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
