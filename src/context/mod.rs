use std::{path::PathBuf, time::Duration};

use derive_more::From;
use leucite::{MemorySize, Rules};

pub mod builder;
use builder::{TestContextBuilder, UnsetRunCmd, UnsetTests};

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

/// A test case which has an input and expected output
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TestCase {
    input: String,
    output: ExpectedOutput,
}

impl TestCase {
    /// Create a new test case from input and output
    pub fn new(input: impl Into<String>, output: impl Into<ExpectedOutput>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
        }
    }
}

/// Configuration for how a file should be setup for test cases to be run
#[derive(Clone, Debug)]
pub struct FileConfig {
    /// This path is relative to the temporary directory created while running tests
    dst: PathBuf,
    src: FileContent,
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

    pub fn bytes(path: impl Into<Box<[u8]>>) -> Self {
        Self::Bytes(path.into())
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
            CommandConfig::Compile(c) => Some(&c),
            CommandConfig::Run(_) => None,
            CommandConfig::Equal(c) => Some(&c),
            CommandConfig::Different { compile, run: _ } => Some(&compile),
        }
    }

    pub fn run(&self) -> Option<&T> {
        match self {
            CommandConfig::None => None,
            CommandConfig::Compile(_) => None,
            CommandConfig::Run(r) => Some(&r),
            CommandConfig::Equal(r) => Some(&r),
            CommandConfig::Different { compile: _, run } => Some(&run),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestContext {
    trim_output: bool,
    files: Vec<FileConfig>,
    command: CommandConfig<Box<[String]>>,
    timeout: CommandConfig<Duration>,
    rules: CommandConfig<Rules>,
    max_memory: CommandConfig<MemorySize>,
    max_file_size: CommandConfig<MemorySize>,
    max_threads: CommandConfig<u64>,
}

impl TestContext {
    pub fn builder() -> TestContextBuilder<UnsetTests, UnsetRunCmd> {
        TestContextBuilder::new()
    }
}
