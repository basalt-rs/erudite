use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use derive_more::From;
use leucite::{MemorySize, Rules};

pub mod builder;
use builder::{MissingRunCmd, MissingTests, TestContextBuilder};

use crate::runner::TestRunner;

#[derive(Debug, Clone, Hash, Eq, PartialEq, From)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExpectedOutput {
    String(#[from] String),
}

impl From<&str> for ExpectedOutput {
    fn from(value: &str) -> Self {
        ExpectedOutput::String(value.into())
    }
}

// TODO: output validator trait?
#[derive(Debug, Clone, Hash, Eq, PartialEq, From)]
pub struct OutputValidator {
    pub(crate) trim_output: bool,
    pub(crate) match_case: bool,
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
            ExpectedOutput::String(ref s) if self.match_case => s.eq_ignore_ascii_case(output),
            ExpectedOutput::String(ref s) => s == output,
        }
    }
}

/// A test case which has an input and expected output
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TestCase<T> {
    pub input: String,
    pub output: ExpectedOutput,
    pub data: T,
}

impl<T> TestCase<T> {
    /// Create a new test case from input and output
    pub fn new(input: impl Into<String>, output: impl Into<ExpectedOutput>, data: T) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            data,
        }
    }
}

/// Configuration for how a file should be setup for test cases to be run
#[derive(Clone, Debug)]
pub struct FileConfig {
    /// This path is relative to the temporary directory created while running tests
    dest: PathBuf,
    src: FileContent,
}

impl FileConfig {
    pub async fn write_file(&self, base: &Path) -> std::io::Result<u64> {
        let target = base.join(&self.dest);
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

#[derive(Clone, Debug)]
pub enum FileContent {
    /// Copies a file directly from this path
    ///
    /// NOTE: This happens when the tests are actually run.  If you want to load the file into
    /// memory first, use [`FileContent::Bytes`].
    Path(PathBuf),
    /// Creates a new file with this content
    Bytes(Box<[u8]>),
}

impl FileContent {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    pub fn string(string: impl Into<String>) -> Self {
        let string: String = string.into();
        Self::bytes(string.into_boxed_str())
    }

    pub fn bytes(bytes: impl Into<Box<[u8]>>) -> Self {
        Self::Bytes(bytes.into())
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CommandConfig<T> {
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
    pub fn new() -> Self {
        Default::default()
    }

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
    pub fn builder() -> TestContextBuilder<MissingTests, MissingRunCmd, T> {
        TestContextBuilder::new()
    }

    pub fn test_builder(self: Arc<Self>) -> TestRunner<'static, T> {
        TestRunner::new(self)
    }
}
