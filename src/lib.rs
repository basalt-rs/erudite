use std::{
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use anyhow::{bail, ensure, Context};
use tmpdir::TmpDir;
use tokio::{fs, io::AsyncWriteExt, process::Command, task::JoinSet};

/// Represents the output of a [`Runner`]'s execution.
#[derive(Debug, Eq, PartialEq)]
pub enum RunOutput {
    /// The runner failed to spawn the command to compile the program
    CompileSpawnFail(String),
    /// The runner failed to compile the command due (compile command had non-zero exit code)
    CompileFail(SimpleOutput),
    /// The _runner_ ran successfully.  This does not necessarily mean the tests passed.
    RunSuccess(Vec<TestOutput>),
}

/// A test case which has an input and expected output
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct TestCase {
    input: String,
    output: String,
}

impl TestCase {
    /// Create a new test case from input and output
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
        }
    }
}

/// Represents some data that may either be a string or a series of bytes.  The recommended method
/// for constructing this type is to use [`From::from`] which will automatically choose the
/// appropriate variant for the data.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Bytes {
    String(String),
    Bytes(Vec<u8>),
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
pub struct SimpleOutput {
    pub stdout: Bytes,
    pub stderr: Bytes,
    pub status: i32,
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

/// The reason that a test may fail
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TestFailReason {
    /// The test took longer than the timeout allowed
    Timeout,
    /// The test returned output that is different from the expected output
    IncorrectOutput(SimpleOutput),
    /// The test exited with a non-zero exit code
    Crash(SimpleOutput),
}

/// A Result-like enum that represents a passed or failed test
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TestOutput {
    Pass,
    Fail(TestFailReason),
}

impl From<Result<(), TestFailReason>> for TestOutput {
    fn from(value: Result<(), TestFailReason>) -> Self {
        match value {
            Ok(()) => Self::Pass,
            Err(r) => Self::Fail(r),
        }
    }
}

/// Configuration for how a file should be setup for test cases to be run
#[derive(Clone, Debug)]
enum FileSetup {
    Copy { dest: PathBuf, src: PathBuf },
    Create { dest: PathBuf, content: Box<[u8]> },
}

/// Convert a slice of strings to a [`Command`] by taking the first one as the program and the rest
/// as args
fn command_from_slice(args: &[String]) -> Option<Command> {
    let mut args = args.iter();
    let mut cmd = Command::new(args.next()?);
    cmd.args(args);
    Some(cmd)
}

/// Lightweight configuration that can implement copy since it will need to be moved
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct CopyConfig {
    timeout: Duration,
    trim_output: bool,
}

impl Default for CopyConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            trim_output: true,
        }
    }
}

/// Runner builder for a suite of tests
///
/// ```no_run
/// # // no_run because `.run()` executes commands
/// # use erudite::Runner;
/// # async fn foo() -> anyhow::Result<()> {
/// let output = Runner::new()
///     .compile_command(["ghc", "solution.hs"])
///     .run_command(["./solution"])
///     .test("hello", "olleh")
///     .copy_file("solution.hs", "solutions/solution.hs")
///     .run()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct Runner {
    copy_config: CopyConfig,
    cwd: Option<PathBuf>,
    files: Vec<FileSetup>,
    compile_command: Option<Vec<String>>,
    run_command: Vec<String>,
    test_cases: Vec<TestCase>,
}

impl Runner {
    /// Create a new runner with default values.
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the command used to compile a solution for this runner.  If the compile command is not
    /// specified, the compilation step will be skipped when calling [`Runner::run`]
    ///
    /// This command is run in a custom directory that may be specified via this builder.  See
    /// [`Runner::run_directory`] for more details.
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// runner.compile_command(["javac", "Solution.java"]);
    /// ```
    pub fn compile_command(
        &mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        self.compile_command = Some(command.into_iter().map(Into::into).collect());
        self
    }

    /// Set the command used to run a solution.  If the run command is not specified, an error will
    /// be returned upon a call to [`Runner::run`]
    ///
    /// This command is run in a custom directory that may be specified via this builder.  See
    /// [`Runner::run_directory`] for more details.
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// runner.run_command(["java", "Solution"]);
    /// ```
    pub fn run_command(
        &mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        self.run_command = command.into_iter().map(Into::into).collect();
        self
    }

    /// Add a single test with expected input and expected output
    ///
    /// If no tests are specified, an error is returned from [`Runner::run`].
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// runner.test("hello", "olleh")
    ///       .test("hello world", "dlrow olleh");
    /// ```
    pub fn test(&mut self, input: impl Into<String>, output: impl Into<String>) -> &mut Self {
        self.test_cases.push(TestCase::new(input, output));
        self
    }

    /// Add multiple tests to the list of test cases for this runner
    ///
    /// If no tests are specified, an error is returned from [`Runner::run`].
    ///
    /// ```
    /// # use erudite::{Runner, TestCase};
    /// let mut runner = Runner::new();
    /// runner.tests([
    ///     TestCase::new("hello", "olleh"),
    ///     TestCase::new("hello world", "dlrow olleh")
    /// ]);
    /// ```
    pub fn tests(&mut self, cases: impl IntoIterator<Item = TestCase>) -> &mut Self {
        self.test_cases.extend(cases);
        self
    }

    /// Determines whether the output should be trimmed before comparison with the expected output
    /// (via [`slice::trim_ascii`])
    ///
    /// Default: `true`
    pub fn trim_output(&mut self, trim_output: bool) -> &mut Self {
        self.copy_config.trim_output = trim_output;
        self
    }

    /// Set the maximum amount of time that any test is allowed to run
    ///
    /// Note: this is for each test individually, not the whole suite
    ///
    /// Default: 1 minute
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.copy_config.timeout = timeout;
        self
    }

    /// Set the directory in which the tests will be run.  If not specified, a directory will be
    /// created in [`std::env::temp_dir`].
    ///
    /// The program will be granted read/write/execute permissions in this directory
    pub fn run_directory(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(path.into());
        self
    }

    /// Designate a file to be created in the run directory with specific content
    ///
    /// The path is based in the directory created in [`Runner::run_directory`].
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// runner.create_file("solution.py", r#"print("hello from python")"#);
    /// ```
    pub fn create_file(
        &mut self,
        path: impl Into<PathBuf>,
        content: impl AsRef<[u8]>,
    ) -> &mut Self {
        self.files.push(FileSetup::Create {
            dest: path.into(),
            content: content.as_ref().to_vec().into_boxed_slice(),
        });
        self
    }

    /// Designate a file to be copied into the run directory from a given location
    ///
    /// The destination path is based in the directory created in [`Runner::run_directory`].  The
    /// source path is based on the current directory of the caller of this library.
    ///
    /// ```
    /// # use erudite::Runner;
    /// let mut runner = Runner::new();
    /// runner.copy_file("solution.hs", "example/solution.hs");
    /// ```
    pub fn copy_file(&mut self, dest: impl Into<PathBuf>, source: impl Into<PathBuf>) -> &mut Self {
        self.files.push(FileSetup::Copy {
            dest: dest.into(),
            src: source.into(),
        });
        self
    }

    /// Validate that this runner is in a valid state to be run
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.run_command.len() != 0, "No run command provided");
        ensure!(self.test_cases.len() != 0, "No test cases provided");
        Ok(())
    }

    /// Create the files requested by [`Runner::create_file`] and [`Runner::copy_file`].
    async fn create_files(&mut self) -> anyhow::Result<(PathBuf, Option<TmpDir>)> {
        let (cwd, tmpdir) = if let Some(cwd) = &self.cwd {
            (cwd, None)
        } else {
            let tmpdir = TmpDir::new(env!("CARGO_CRATE_NAME"))
                .await
                .context("Creating temp dir")?;
            (&tmpdir.to_path_buf(), Some(tmpdir))
        };

        fs::create_dir_all(cwd)
            .await
            .with_context(|| format!("Creating dir '{}'", cwd.display()))?;

        for file in &self.files {
            match file {
                FileSetup::Copy { dest, src } => {
                    let mut full_dest = cwd.clone();
                    full_dest.extend(dest);
                    eprintln!("Copying {} -> {}", src.display(), full_dest.display());
                    fs::copy(src, &full_dest).await.with_context(|| {
                        format!("Copying {} -> {}", src.display(), full_dest.display())
                    })?;
                }
                FileSetup::Create { dest, content } => {
                    let mut full_dest = cwd.clone();
                    full_dest.extend(dest);
                    eprintln!(
                        "Creating file {} with {} bytes",
                        full_dest.display(),
                        content.len()
                    );
                    fs::write(&full_dest, content).await.with_context(|| {
                        format!(
                            "Creating file {} with {} bytes",
                            full_dest.display(),
                            content.len()
                        )
                    })?;
                }
            }
        }

        Ok((cwd.to_path_buf(), tmpdir))
    }

    /// Run a given test
    ///
    /// This function is called in [`tokio::spawn`], so doesn't take `self`.
    async fn run_test(
        mut run_command: Command,
        copy_config: CopyConfig,
        case: TestCase,
    ) -> anyhow::Result<TestOutput> {
        run_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = run_command
            // TODO: Use `leucite` to restrict this command
            .spawn()
            .with_context(|| format!("running command {:?}", run_command))?;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(case.input.as_bytes())
            .await
            .context("Writing stdin to test case")?;
        drop(stdin);
        let out = match tokio::time::timeout(copy_config.timeout, child.wait_with_output()).await {
            Ok(out) => out?,
            Err(_) => {
                return Ok(TestOutput::Fail(TestFailReason::Timeout));
            }
        };

        let (expected, actual) = if copy_config.trim_output {
            (case.output.as_bytes().trim_ascii(), out.stdout.trim_ascii())
        } else {
            (case.output.as_bytes(), &out.stdout[..])
        };

        if !out.status.success() {
            Ok(TestOutput::Fail(TestFailReason::Crash(out.into())))
        } else if expected == actual {
            Ok(TestOutput::Pass)
        } else {
            Ok(TestOutput::Fail(TestFailReason::IncorrectOutput(
                out.into(),
            )))
        }
    }

    /// # Panics:
    ///
    /// - If `self.config.compile_command` is not a valid command
    async fn compile(&self, cwd: &Path) -> Result<(), RunOutput> {
        let Some(compile_command) = &self.compile_command else {
            // There is no compile command, and thus no compile step needed
            return Ok(());
        };

        let output = command_from_slice(compile_command)
            .expect("Checked by caller")
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // TODO: Use `leucite` to restrict this command
            .spawn()
            .map_err(|e| RunOutput::CompileSpawnFail(e.to_string()))?
            .wait_with_output()
            .await
            .map_err(|e| RunOutput::CompileSpawnFail(e.to_string()))?;

        if !output.status.success() {
            return Err(RunOutput::CompileFail(output.into()));
        }

        Ok(())
    }

    async fn run_tests(&self, cwd: &Path) -> anyhow::Result<RunOutput> {
        // outputs of each test
        // Keeping a list like this so we maintain order.
        let mut out: Vec<Option<TestOutput>> = vec![None; self.test_cases.len()];

        let mut joinset = JoinSet::new();
        for (i, case) in self.test_cases.clone().into_iter().enumerate() {
            let Some(mut run_command) = command_from_slice(&self.run_command) else {
                bail!("Invalid command {:?}", self.run_command);
            };
            run_command.current_dir(cwd);
            // copy the configs before we pass them into the `spawn` call.
            let copy = self.copy_config;
            joinset.spawn(async move {
                Self::run_test(run_command, copy, case)
                    .await
                    .map(|v| (i, v))
            });
        }

        while let Some(res) = joinset.join_next().await {
            let (i, res) = res
                .context("joining on command output")?
                .context("test output")?;
            out[i] = Some(res);
        }

        let out: Vec<_> = out.into_iter().filter_map(|t| t).collect();

        assert!(
            out.len() == self.test_cases.len(),
            "Out should have every case after joinset is done"
        );

        Ok(RunOutput::RunSuccess(out))
    }

    /// Build and run all tests associated with this runner
    ///
    /// Will error if the runner is not in a valid state or an error occurs while setting up or
    /// cleaning up the tests.
    ///
    /// Any output related to the execution of the tests will be returned throught the
    /// [`RunOutput`]
    pub async fn run(&mut self) -> anyhow::Result<RunOutput> {
        self.validate()?;
        let (cwd, mut tmpdir) = self.create_files().await.context("Creating files")?;

        if let Err(e) = self.compile(&cwd).await {
            return Ok(e);
        }

        let output = self.run_tests(&cwd).await.context("running test suite")?;

        // We should not need to delete the tmpdir manually, but it doesn't seem to work without us
        // manually calling `tmpdir.close()`.
        if let Some(tmpdir) = tmpdir.take() {
            tmpdir.close().await.with_context(|| {
                format!("Deleting temp dir '{}'", tmpdir.to_path_buf().display())
            })?;
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use crate::Runner;

    #[tokio::test]
    async fn missing_fields() {
        let ret = Runner::new().run().await;
        assert!(matches!(ret, Err(_)));
    }

    #[tokio::test]
    async fn missing_run_command() {
        let ret = Runner::new().test("hello", "olleh").run().await;
        assert!(matches!(ret, Err(_)));
    }

    #[tokio::test]
    async fn missing_test_cases() {
        let ret = Runner::new()
            .run_command(["node", "solution.js"])
            .run()
            .await;
        assert!(matches!(ret, Err(_)));
    }
}
