use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use leucite::{CommandExt, Rules};
use tmpdir::TmpDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    task::JoinSet,
    time::Instant,
    try_join,
};
use tracing::{debug, debug_span, instrument, trace, Instrument};

use crate::{
    cases::{OutputValidator, TestCase},
    context::{CommandConfig, TestContext},
    error::{CompileError, CreateFilesError, SpawnTestError},
    Bytes, Output,
};

struct TestFileConfig<'a> {
    /// This path is relative to the temporary directory created while running tests
    dest: &'a Path,
    src: TestFileContent<'a>,
}

impl TestFileConfig<'_> {
    async fn write_file(&mut self, base: &Path) -> std::io::Result<u64> {
        let target = base.join(self.dest);
        match self.src.0 {
            TestFileContentInner::Path(ref path) => tokio::fs::copy(path, target).await,
            TestFileContentInner::Bytes(ref contents) => tokio::fs::write(target, contents)
                .await
                .map(|_| contents.len() as _),
            TestFileContentInner::Reader(ref mut reader) => {
                let mut out = tokio::fs::File::open(target).await?;
                tokio::io::copy(reader, &mut out).await
            }
        }
    }

    pub fn dest(&self) -> &Path {
        self.dest
    }
}

/// A trait which is added to all [`AsyncRead`] + [`Unpin`]
#[doc(hidden)]
pub trait AsyncReadUnpin: AsyncRead + Unpin {}
impl<T> AsyncReadUnpin for T where T: AsyncRead + Unpin {}

/// Some form of content that will be used to create a file when a test is compiled
pub struct TestFileContent<'a>(TestFileContentInner<'a>);
// NOTE: This enum is wrapped so that it can't be created directly by a consuming library
enum TestFileContentInner<'a> {
    /// Copies a file directly from this path
    ///
    /// NOTE: This happens when the tests are actually run.  If you want to load the file into
    /// memory first, use [`Self::Bytes`].
    Path(&'a Path),
    /// Creates a new file with this content
    Bytes(&'a [u8]),
    Reader(&'a mut dyn AsyncReadUnpin),
}

impl<'a> TestFileContent<'a> {
    /// Copy the file directly from this path on the host machine
    ///
    /// Note: The copy happens when tests are compiled.  If you want to load the file into memory
    /// first, use [`TestFileContent::bytes`].
    pub fn path(path: &'a Path) -> Self {
        Self(TestFileContentInner::Path(path))
    }

    /// Write a string to the file
    pub fn string(string: &'a str) -> Self {
        Self(TestFileContentInner::Bytes(string.as_bytes()))
    }

    /// Write bytes to the file
    pub fn bytes(bytes: &'a [u8]) -> Self {
        Self(TestFileContentInner::Bytes(bytes))
    }

    /// Copy from this reader into the file
    pub fn reader<R: AsyncRead + Unpin>(r: &'a mut R) -> Self {
        Self(TestFileContentInner::Reader(r))
    }
}

impl<'a> From<&'a Path> for TestFileContent<'a> {
    fn from(value: &'a Path) -> Self {
        Self::path(value)
    }
}

/// Parse a command from argv.  `argv[0]` is the program, `argv[1..]` is the args.
/// Returns `None` if the command is not valid.
fn command_from_argv(argv: &[String]) -> Option<Command> {
    let (program, args) = argv.split_first()?;

    let mut cmd = Command::new(program);
    cmd.args(args);
    Some(cmd)
}

/// Similar to `Child::wait_with_output`, but with a few changes:
/// - Writes `input` into STDIN if `input` is Some
/// - Spawns child with a timeout
/// - Collects output from the child, even if the timeout happens
async fn wait_with_output_and_timeout(
    child: &mut Child,
    timeout: Option<Duration>,
    input: Option<&str>,
) -> std::io::Result<(Output, bool)> {
    let stdin_input = if let Some(input) = input {
        Some((child.stdin.take().expect("We only take this once"), input))
    } else {
        None
    };
    let stdout_pipe = child.stdout.take().expect("We only take this once");
    let stderr_pipe = child.stderr.take().expect("We only take this once");

    async fn read_to_end<R>(mut r: R, stdout: bool) -> std::io::Result<Vec<u8>>
    where
        R: AsyncRead + Unpin,
    {
        let mut out = Vec::new();
        let bytes = r.read_to_end(&mut out).await?;
        trace!(
            bytes,
            "finished reading from {}",
            if stdout { "stdout" } else { "stderr" }
        );
        Ok(out)
    }

    async fn write_input<W>(mut w: W, input: impl AsRef<[u8]>) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let input = input.as_ref();
        let mut bytes = 0;
        w.write_all(input).await?;
        bytes += input.len();
        // Required for empty input to work well in some languages
        w.write_u8(b'\n').await?;
        bytes += 1;
        trace!(bytes, "finished writing to stdin");
        Ok(())
    }

    let stdin_fut = async move {
        if let Some((stdin_pipe, input)) = stdin_input {
            write_input(stdin_pipe, input).await
        } else {
            Ok(())
        }
    };
    let stdout_fut = read_to_end(stdout_pipe, true);
    let stderr_fut = read_to_end(stderr_pipe, false);
    let wait_fut = async move {
        trace!("waiting on test child");
        let (timed_out, exit_status) = if let Some(timeout) = timeout {
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(exit_status)) => {
                    trace!("test ran and successfully waited");
                    (false, exit_status.code().unwrap_or(1))
                }
                Ok(Err(e)) => {
                    trace!("test ran, but failed while waiting");
                    return Err(e);
                }
                Err(elapsed) => {
                    trace!(?elapsed, "test timed out");
                    child.kill().await?;
                    (true, 0)
                }
            }
        } else {
            let exit_status = child.wait().await.map(|x| x.code().unwrap_or(1))?;
            (false, exit_status)
        };
        Ok((timed_out, exit_status))
    };

    let ((), stdout, stderr, (timed_out, exit_status)) =
        try_join!(stdin_fut, stdout_fut, stderr_fut, wait_fut)?;

    Ok((Output::new(stdout, stderr, exit_status), timed_out))
}

/// A suite of tests that are about to be run.  This can be created from the
/// [`TestContext::test_runner`] function and will inherit the configuration from the context.  
///
/// ```no_run
/// # use std::{path::Path, sync::Arc};
/// # use erudite::context::TestContext;
/// # #[derive(Clone)]
/// # struct Data { visible: bool }
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let context = TestContext::builder()
///     .test("hello", "olleh", Data { visible: false })
///     .test("world", "dlrow", Data { visible: true })
///     .run_command(["node", "solution.js"])
///     .build();
/// let context = Arc::new(context);
///
/// let test_handle = context.test_runner()
///     .file(Path::new("user-solution.js"), Path::new("solution.js"))
///     .filter_tests(|t| t.data().visible)
///     .cwd(Path::new("./test"))
///     .collect_output(true)
///     .compile_and_run()
///     .await?;
/// # Ok(()) }
/// ```
#[must_use]
pub struct TestRunner<'a, T> {
    context: Arc<TestContext<T>>,
    files: Vec<TestFileConfig<'a>>,
    test_filter: Option<fn(&TestCase<T>) -> bool>,
    cwd: Option<&'a Path>,
    collect_output: bool,
}

/// Builder functions
impl<'a, T> TestRunner<'a, T> {
    pub(crate) fn new(context: Arc<TestContext<T>>) -> Self {
        Self {
            context,
            files: Default::default(),
            test_filter: None,
            cwd: None,
            collect_output: true,
        }
    }

    /// Add a file to this test runner.  This file will be added before the test is compiled.
    pub fn file(mut self, source: impl Into<TestFileContent<'a>>, dest: &'a Path) -> Self {
        self.files.push(TestFileConfig {
            dest,
            src: source.into(),
        });
        self
    }

    /// Add a filter to run only a subset of the test cases.  Any case which returns `true` from
    /// this filter will be run.
    pub fn filter_tests(mut self, filter: fn(&TestCase<T>) -> bool) -> Self {
        self.test_filter = Some(filter);
        self
    }

    /// Set the current working directory in which the compile and run commands are executed.  If
    /// not specified, this will create a temporary directory.
    pub fn cwd(mut self, cwd: &'a Path) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Whether the test runner should collect the output of the compiler.  If this is set to
    /// `false`, the `stdout` and `stderr` fields in [`CompileResult`] will both be empty.  This
    /// does not affect the `exit_status`.
    ///
    /// Default = `true`
    pub fn collect_output(mut self, collect_output: bool) -> Self {
        self.collect_output = collect_output;
        self
    }
}

// implementation functions
impl<'a, T> TestRunner<'a, T>
where
    T: Send + Sync + Clone + 'static,
{
    async fn create_files(&mut self, cwd: &Path) -> Result<(), CreateFilesError> {
        for file in &self.context.files {
            file.write_file(cwd)
                .await
                .map_err(|error| CreateFilesError {
                    path: cwd.join(file.dest()),
                    error,
                })?;
        }

        for file in &mut self.files {
            file.write_file(cwd)
                .await
                .map_err(|error| CreateFilesError {
                    path: cwd.join(file.dest()),
                    error,
                })?;
        }

        Ok(())
    }

    async fn compile_impl(
        &mut self,
        cwd: &Path,
        compile_rules: Option<Arc<Rules>>,
    ) -> Result<Option<CompileResult>, CompileError> {
        let Some(compile_command) = self.context.command.compile() else {
            // There is no compile command, and thus no compile step needed
            return Ok(None);
        };

        let start = Instant::now();
        let mut child = command_from_argv(compile_command)
            .ok_or(CompileError::InvalidCommand)?
            .current_dir(cwd)
            // TODO: write output to temp files, rather than collecting in memory
            .stdout(if self.collect_output {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(if self.collect_output {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .restrict_if(compile_rules)
            .max_memory_if(self.context.max_memory.run().copied())
            .max_file_size_if(self.context.max_file_size.run().copied())
            .max_threads_if(self.context.max_threads.run().copied())
            .spawn()
            .map_err(CompileError::SpawnFail)?;

        let (output, timed_out) =
            wait_with_output_and_timeout(&mut child, self.context.timeout.compile().copied(), None)
                .await
                .map_err(CompileError::WaitFail)?;

        let state = if timed_out {
            CompileResultState::TimedOut
        } else if output.success() {
            CompileResultState::Success
        } else {
            CompileResultState::RuntimeFail
        };

        let time_taken = start.elapsed();

        Ok(Some(CompileResult {
            output,
            state,
            time_taken,
        }))
    }

    fn create_rules(&self, cwd: &Path) -> (Option<Arc<Rules>>, Option<Arc<Rules>>) {
        let modify_rules = |rules: Rules| -> Rules { rules.clone().add_read_write(cwd) };
        match &self.context.rules {
            CommandConfig::None => (None, None),
            CommandConfig::Compile(ref r) => (Some(Arc::new(modify_rules(r.clone()))), None),
            CommandConfig::Run(ref r) => (None, Some(Arc::new(modify_rules(r.clone())))),
            CommandConfig::Equal(ref r) => {
                // Done this way to only create once instance of the rules and just Arc::clone it
                let r = modify_rules(r.clone());
                let r = Arc::new(r);
                (Some(Arc::clone(&r)), Some(r))
            }
            CommandConfig::Different { compile, run } => (
                Some(Arc::new(modify_rules(compile.clone()))),
                Some(Arc::new(modify_rules(run.clone()))),
            ),
        }
    }

    /// Create the environment for the test and compile the solution.  This will make the temp
    /// directory if neccessary, write any files added via [`TestRunner::file`], and compile the
    /// solution using the compile command.
    ///
    /// If this runner does not have a compile step, all of the steps listed above, except for the
    /// compilation, will still be completed.
    ///
    /// ```no_run
    /// # use std::{path::Path, sync::Arc};
    /// # use erudite::context::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("hello", "olleh", Data { visible: false })
    ///     .test("world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let compiled = context.test_runner()
    ///     .file(Path::new("user-solution.js"), Path::new("solution.js"))
    ///     .filter_tests(|t| t.data().visible)
    ///     .cwd(Path::new("./test"))
    ///     .collect_output(true)
    ///     .compile()
    ///     .await?;
    ///
    /// let test_handle = compiled.run();
    /// # Ok(()) }
    /// ```
    pub async fn compile(mut self) -> Result<CompiledTestRunner<'a, T>, CompileError> {
        let mut tmpdir = None;
        let cwd = if let Some(cwd) = self.cwd.take() {
            trace!(?cwd, "Using specified cwd");
            cwd.to_path_buf()
        } else {
            trace!("Creating temp dir");
            tmpdir = Some(
                TmpDir::new("erudite")
                    .await
                    .map_err(CompileError::MktempFail)?,
            );
            tmpdir
                .as_ref()
                .expect("we literally just assigned it")
                .to_path_buf()
        };

        debug!(path = ?cwd, "setting up directory");

        let (compile_rules, run_rules) = self.create_rules(&cwd);

        trace!("creating files");
        let start = Instant::now();
        self.create_files(&cwd).await?;
        let elapsed = start.elapsed();
        debug!(in = ?elapsed, "created files");

        debug!(?cwd, "starting compilation");
        let start = Instant::now();
        let compile_output = self.compile_impl(&cwd, compile_rules).await?;
        let elapsed = start.elapsed();
        debug!(in = ?elapsed, "finished compilation");

        if compile_output
            .as_ref()
            .is_some_and(|c| c.state() == CompileResultState::RuntimeFail)
        {
            let compile_output = compile_output.unwrap();
            Err(CompileError::CompileFail(compile_output))
        } else {
            Ok(CompiledTestRunner {
                test_runner: self,
                run_rules,
                cwd,
                tmpdir,
                compile_result: compile_output,
            })
        }
    }

    /// Create the environment for the test and compile the solution.  This will make the temp
    /// directory if neccessary, write any files added via [`TestRunner::file`], compile the
    /// solution using the compile command, and then spawn the tests.
    ///
    /// If this runner does not have a compile step, all of the steps listed above, except for the
    /// compilation, will still be completed.
    ///
    /// ```no_run
    /// # use std::{path::Path, sync::Arc};
    /// # use erudite::context::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("hello", "olleh", Data { visible: false })
    ///     .test("world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let test_handle = context.test_runner()
    ///     .file(Path::new("user-solution.js"), Path::new("solution.js"))
    ///     .filter_tests(|t| t.data().visible)
    ///     .cwd(Path::new("./test"))
    ///     .collect_output(true)
    ///     .compile_and_run()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn compile_and_run(self) -> Result<TestHandle<T>, CompileError> {
        Ok(self.compile().await?.run())
    }
}

/// A test runner that has been compiled already.  The tests are now able to be run.
#[must_use]
pub struct CompiledTestRunner<'a, T> {
    test_runner: TestRunner<'a, T>,
    cwd: PathBuf,
    tmpdir: Option<TmpDir>,
    compile_result: Option<CompileResult>,
    run_rules: Option<Arc<Rules>>,
}

impl<T> CompiledTestRunner<'_, T> {
    /// Get the result of the compilation step.  This also available from [`TestHandle::compile_result`].
    pub fn compile_result(&self) -> Option<&CompileResult> {
        self.compile_result.as_ref()
    }
}

impl<T> CompiledTestRunner<'_, T>
where
    T: Send + Sync + Clone + 'static,
{
    async fn run_test(
        index: usize,
        cwd: &Path,
        case: TestCase<T>,
        run_rules: Option<Arc<Rules>>,
        context: Arc<TestContext<T>>,
    ) -> Result<TestResult<T>, SpawnTestError> {
        let run_command = context.command.run().expect("checked in builder");
        let start = Instant::now();
        let mut child = command_from_argv(run_command)
            .ok_or(SpawnTestError::InvalidCommand)?
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .restrict_if(run_rules)
            .max_memory_if(context.max_memory.run().copied())
            .max_file_size_if(context.max_file_size.run().copied())
            .max_threads_if(context.max_threads.run().copied())
            .spawn()
            .map_err(SpawnTestError::SpawnFail)?;

        let (output, timed_out) = wait_with_output_and_timeout(
            &mut child,
            context.timeout.run().copied(),
            Some(&case.input),
        )
        .await
        .map_err(SpawnTestError::WaitFail)?;

        let time_taken = start.elapsed();

        let validator = OutputValidator {
            trim_output: context.trim_output,
            expected_output: case.output,
        };

        let state = if timed_out {
            TestResultState::TimedOut
        } else if output.status != 0 {
            TestResultState::RuntimeFail
        } else if let Some(stdout) = output.stdout.as_str() {
            if validator.is_valid(stdout) {
                TestResultState::Pass
            } else {
                TestResultState::IncorrectOutput
            }
        } else {
            TestResultState::IncorrectOutput
        };
        Ok(TestResult {
            index,
            data: Some(case.data),
            output,
            state,
            time_taken,
        })
    }

    fn spawn_tests(&mut self) -> JoinSet<Result<TestResult<T>, SpawnTestError>> {
        let mut joinset = JoinSet::new();

        for (i, case) in self.test_runner.context.test_cases.iter().enumerate() {
            if self
                .test_runner
                .test_filter
                .is_some_and(|filter| !filter(case))
            {
                trace!(?case.input, ?case.output, "Skipping test");
                continue;
            }

            let case = case.clone();
            const MAX_LEN: usize = 10;
            let input = if case.input.len() > MAX_LEN {
                &format!("{}…", &case.input[..MAX_LEN])
            } else {
                &case.input
            };
            let span = debug_span!("run_test", index = i, ?input);
            let context = Arc::clone(&self.test_runner.context);
            let run_rules = self.run_rules.as_ref().map(Arc::clone);
            let path = self.cwd.to_path_buf();
            joinset.spawn(
                async move { Self::run_test(i, &path, case, run_rules, context).await }
                    .instrument(span),
            );
        }

        joinset
    }

    /// Start all of the tests and returns a handle which can wait on test completion.  See
    /// [`TestHandle`].
    ///
    /// ```no_run
    /// # use std::{path::Path, sync::Arc};
    /// # use erudite::context::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("hello", "olleh", Data { visible: false })
    ///     .test("world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let compiled = context.test_runner()
    ///     .file(Path::new("user-solution.js"), Path::new("solution.js"))
    ///     .filter_tests(|t| t.data().visible)
    ///     .cwd(Path::new("./test"))
    ///     .collect_output(true)
    ///     .compile()
    ///     .await?;
    ///
    /// let test_handle = compiled.run();
    /// # Ok(()) }
    /// ```
    #[instrument(skip(self))]
    pub fn run(mut self) -> TestHandle<T> {
        trace!("spawning tests");
        let tests = self.spawn_tests();

        TestHandle {
            joinset: tests,
            test_count: self.test_runner.context.test_cases.len(),
            compile_result: self.compile_result,
            _tmpdir: self.tmpdir,
        }
    }
}

/// The state of a test result
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TestResultState {
    /// This test has passed without issue
    Pass,
    /// This test failed while running (exit status != 0)
    RuntimeFail,
    /// This test timed out
    TimedOut,
    /// This test printed the incorrect output
    IncorrectOutput,
}

/// The result from running the test.  This also contains the data associated with the test and the
/// index of the test when added to the [`TestContext`].
#[derive(Debug)]
pub struct TestResult<T> {
    index: usize,
    /// Option so that it can be taken using [`Self::take_data`]
    data: Option<T>,
    output: Output,
    state: TestResultState,
    time_taken: Duration,
}

impl<T> TestResult<T> {
    /// The index of the test as added in [`TestContext`].
    pub fn index(&self) -> usize {
        self.index
    }

    /// Take the data associated with this test.  After the first call, will always return [`None`]..
    pub fn take_data(&mut self) -> Option<T> {
        self.data.take()
    }

    /// Get the data associated with this test.  Returns `None` if the data has been taken via
    /// [`TestResult::take_data`].
    ///
    /// See also [`TestResult::take_data`] if the data needs to be owned.
    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    /// Get the time taken by this test case
    pub fn time_taken(&self) -> Duration {
        self.time_taken
    }

    /// Get the state of this test result
    pub fn state(&self) -> TestResultState {
        self.state
    }

    /// Get the output (stdout/stderr/exit status) of this test
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Get the standard output of this test
    pub fn stdout(&self) -> &Bytes {
        &self.output.stdout
    }

    /// Get the standard error of this test
    pub fn stderr(&self) -> &Bytes {
        &self.output.stderr
    }

    /// Get the exit status of this test
    pub fn exit_status(&self) -> i32 {
        self.output.status
    }
}

/// A handle to a set of running tests.  Tests can either be waited on one-at-a-time (using
/// [`TestHandle::wait_next`]), or in bulk (using [`TestHandle::wait_all`]).
///
// TODO: code example
pub struct TestHandle<T> {
    joinset: JoinSet<Result<TestResult<T>, SpawnTestError>>,
    test_count: usize,
    compile_result: Option<CompileResult>,
    /// Needed to remove the temp dir when we're done
    _tmpdir: Option<TmpDir>,
}

impl<T> TestHandle<T> {
    /// Get the quantity of tests that are still running or haven't been collected using
    /// [`TestHandle::wait_next`].
    pub fn tests_left(&self) -> usize {
        self.joinset.len()
    }

    /// Get the total quantity of test cases that this test handle keeps track of
    pub fn test_count(&self) -> usize {
        self.test_count
    }

    /// Get the result of the compilation step from [`TestRunner::compile`].  This returns `None`
    /// if the tests did not need a compile step.
    pub fn compile_result(&self) -> Option<&CompileResult> {
        self.compile_result.as_ref()
    }
}

impl<T: 'static> TestHandle<T> {
    /// Wait for the next test to finish.  The test result returned from this is _not_ ordered, but
    /// the index may be received from [`TestResult::index`].
    pub async fn wait_next(&mut self) -> Result<Option<TestResult<T>>, SpawnTestError> {
        match self.joinset.join_next().await {
            Some(Err(e)) => Err(SpawnTestError::JoinError(e)),
            Some(Ok(v)) => Ok(Some(v?)),
            None => Ok(None),
        }
    }

    /// Wait for all tests to complete and return a vector of test cases.  The returned vector _is
    /// ordered_ based on the order in which the tests were inserted in the [`TestContext`].
    pub async fn wait_all(&mut self) -> Result<Vec<TestResult<T>>, SpawnTestError> {
        // NOTE: using joinset len here, rather than test_count in case `wait_next` has been called at all
        let len = self.joinset.len();
        let mut out = Vec::with_capacity(len);
        let out_slice = out.spare_capacity_mut();
        let mut added = 0;
        while let Some(result) = self.wait_next().await? {
            out_slice[result.index()].write(result);
            added += 1;
        }
        assert_eq!(added, len);
        assert_eq!(added, out.capacity());
        // SAFETY: If added == test_count == capacity, then we have assigned every value within the
        // allocated vector, so the vector has has `len = added`
        unsafe { out.set_len(added) };

        Ok(out)
    }
}

/// The state of the result of the compilation step
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompileResultState {
    /// The compiler exited successfully (exit status == 0)
    Success,
    /// The compiler exited unsuccessfully (exit status != 0)
    RuntimeFail,
    /// The compiler timed out while running
    TimedOut,
}

/// The result from running the compile step
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileResult {
    output: Output,
    state: CompileResultState,
    time_taken: Duration,
}

impl CompileResult {
    /// Get the time taken by the compile step
    pub fn time_taken(&self) -> Duration {
        self.time_taken
    }

    /// Get the state of the compile step
    pub fn state(&self) -> CompileResultState {
        self.state
    }

    /// Get the output (stdout/stderr/exit status)
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Get the standard output
    pub fn stdout(&self) -> &Bytes {
        &self.output.stdout
    }

    /// Get the standard error
    pub fn stderr(&self) -> &Bytes {
        &self.output.stderr
    }

    /// Get the exit status
    pub fn exit_status(&self) -> i32 {
        self.output.status
    }
}
