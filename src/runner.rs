//! Definitions and functions related to the execution of test suites

use std::{
    io::ErrorKind,
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
    cases::TestCase,
    context::StageConfig,
    error::{CompileError, CreateFilesError, SpawnTestError},
    BorrowedFileConfig, BorrowedFileContent, Bytes, Output, TestContext,
};

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
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    async fn read_to_end<R>(r: Option<R>, stdout: bool) -> std::io::Result<Vec<u8>>
    where
        R: AsyncRead + Unpin,
    {
        let Some(mut r) = r else {
            return Ok(Vec::new());
        };
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
        match w.write_all(input).await {
            Ok(()) => {}
            // if pipe is broken, we don't really care
            Err(e) if e.kind() == ErrorKind::BrokenPipe => {
                trace!("Pipe broken while writing stdin");
                return Ok(());
            }
            Err(e) => Err(e)?,
        }
        bytes += input.len();
        // Required for empty input to work well in some languages
        match w.write_u8(b'\n').await {
            Ok(()) => {}
            // if pipe is broken, we don't really care
            Err(e) if e.kind() == ErrorKind::BrokenPipe => {
                trace!("Pipe broken while writing stdin");
                return Ok(());
            }
            Err(e) => Err(e)?,
        }
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
/// # use erudite::TestContext;
/// # #[derive(Clone)]
/// # struct Data { visible: bool }
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let context = TestContext::builder()
///     .test("group", "hello", "olleh", Data { visible: false })
///     .test("group", "world", "dlrow", Data { visible: true })
///     .run_command(["node", "solution.js"])
///     .build();
/// let context = Arc::new(context);
///
/// let test_handle = context
///     .test_runner(&"group")
///     .unwrap()
///     .file(Path::new("user-solution.js"), Path::new("solution.js"))
///     .filter_tests(|t| t.data().visible)
///     .cwd(Path::new("./test"))
///     .collect_output(true)
///     .compile_and_run()
///     .await?;
/// # Ok(()) }
/// ```
#[must_use]
#[derive(Debug)]
pub struct TestRunner<'a, G, T> {
    context: Arc<TestContext<G, T>>,
    files: Vec<BorrowedFileConfig<'a>>,
    test_filter: Option<fn(&TestCase<T>) -> bool>,
    test_cases: Arc<[TestCase<T>]>,
    cwd: Option<&'a Path>,
    collect_output: bool,
}

// Builder functions
impl<'a, G, T> TestRunner<'a, G, T> {
    pub(crate) fn new(context: Arc<TestContext<G, T>>, cases: Arc<[TestCase<T>]>) -> Self {
        Self {
            context,
            files: Default::default(),
            test_filter: None,
            test_cases: cases,
            cwd: None,
            collect_output: true,
        }
    }

    /// Add a file to be inserted into the runtime environment of the test.  This is added before
    /// compilation, so it works well for libraries or input/output manipulation.
    ///
    /// If `source` is a path, the file will only be copied when a test is compiled, meaning
    /// that if the file changes or is removed, then the output may differ between two
    /// instances of the test runner from a single context.
    ///
    /// If the intended behaviour is to read the file _now_, consider reading the file directly
    /// and placing it into [`BorrowedFileContent::bytes`].
    ///
    /// `destination` is path relative to the directory used for the test environment.  If
    /// destination an absolute path, then it will be made relative to the test environment, i.e.,
    /// `/foo/bar` -> `<test-env>/foo/bar`.
    pub fn file(mut self, source: impl Into<BorrowedFileContent<'a>>, dest: &'a Path) -> Self {
        self.files.push(BorrowedFileConfig::new(source, dest));
        self
    }

    /// Add multiple files to be inserted into the runtime environment of the test.  These files
    /// are added before compilation, so it works well for libraries or input/output manipulation.
    ///
    /// If `source` is a path, the file will only be copied when a test is compiled, meaning
    /// that if the file changes or is removed, then the output may differ between two
    /// instances of the test runner from a single context.
    ///
    /// If the intended behaviour is to read the file _now_, consider reading the file directly
    /// and placing it into [`BorrowedFileContent::bytes`].
    ///
    /// `destination` is path relative to the directory used for the test environment.  If
    /// destination an absolute path, then it will be made relative to the test environment, i.e.,
    /// `/foo/bar` -> `<test-env>/foo/bar`.
    pub fn files(
        mut self,
        files: impl IntoIterator<Item = impl Into<BorrowedFileConfig<'a>>>,
    ) -> Self {
        self.files.extend(files.into_iter().map(Into::into));
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
impl<'a, G, T> TestRunner<'a, G, T>
where
    T: Send + Sync + Clone + 'static,
    G: Send + Sync + 'static,
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

    // returns (compile rules, run rules)
    fn create_rules(&self, cwd: &Path) -> (Option<Arc<Rules>>, Option<Arc<Rules>>) {
        let modify_rules = |rules: Rules| -> Rules { rules.clone().add_read_write(cwd) };
        match &self.context.rules {
            StageConfig::None => (None, None),
            StageConfig::Compile(ref r) => (Some(Arc::new(modify_rules(r.clone()))), None),
            StageConfig::Run(ref r) => (None, Some(Arc::new(modify_rules(r.clone())))),
            StageConfig::Equal(ref r) => {
                // Done this way to only create once instance of the rules and just Arc::clone it
                let r = modify_rules(r.clone());
                let r = Arc::new(r);
                (Some(Arc::clone(&r)), Some(r))
            }
            StageConfig::Different { compile, run } => (
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
    /// # use erudite::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("group", "hello", "olleh", Data { visible: false })
    ///     .test("group", "world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let compiled = context
    ///     .test_runner(&"group")
    ///     .unwrap()
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
    pub async fn compile(mut self) -> Result<CompiledTestRunner<'a, G, T>, CompileError> {
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
            .is_some_and(|c| c.state() != CompileResultState::Success)
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
    /// # use erudite::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("group", "hello", "olleh", Data { visible: false })
    ///     .test("group", "world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let test_handle = context
    ///     .test_runner(&"group")
    ///     .unwrap()
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
#[derive(Debug)]
pub struct CompiledTestRunner<'a, G, T> {
    test_runner: TestRunner<'a, G, T>,
    cwd: PathBuf,
    tmpdir: Option<TmpDir>,
    compile_result: Option<CompileResult>,
    run_rules: Option<Arc<Rules>>,
}

impl<G, T> CompiledTestRunner<'_, G, T> {
    /// Get the result of the compilation step.  This also available from [`TestHandle::compile_result`].
    pub fn compile_result(&self) -> Option<&CompileResult> {
        self.compile_result.as_ref()
    }
}

impl<G, T> CompiledTestRunner<'_, G, T>
where
    T: Send + Sync + Clone + 'static,
    G: Send + Sync + 'static,
{
    async fn run_test(
        index: usize,
        cwd: &Path,
        case: TestCase<T>,
        run_rules: Option<Arc<Rules>>,
        context: Arc<TestContext<G, T>>,
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
            Some(case.input()),
        )
        .await
        .map_err(SpawnTestError::WaitFail)?;

        let time_taken = start.elapsed();

        let state = if timed_out {
            TestResultState::TimedOut
        } else if output.status != 0 {
            TestResultState::RuntimeFail
        } else if let Some(stdout) = output.stdout.as_str() {
            let stdout = if context.trim_output {
                stdout.trim()
            } else {
                stdout
            };

            if case.output().is_valid(stdout) {
                TestResultState::Pass
            } else {
                TestResultState::IncorrectOutput
            }
        } else {
            TestResultState::IncorrectOutput
        };
        Ok(TestResult {
            index,
            data: Some(case.into_data()),
            output,
            state,
            time_taken,
        })
    }

    fn spawn_tests(&mut self) -> JoinSet<Result<TestResult<T>, SpawnTestError>> {
        let mut joinset = JoinSet::new();

        for (i, case) in self.test_runner.test_cases.iter().enumerate() {
            if self
                .test_runner
                .test_filter
                .is_some_and(|filter| !filter(case))
            {
                trace!(input = ?case.input(), output = ?case.output(), "Skipping test");
                continue;
            }

            let case = case.clone();
            const MAX_LEN: usize = 10;
            let input = if case.input().len() > MAX_LEN {
                &format!("{}…", &case.input()[..MAX_LEN])
            } else {
                case.input()
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
    /// # use erudite::TestContext;
    /// # #[derive(Clone)]
    /// # struct Data { visible: bool }
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let context = TestContext::builder()
    ///     .test("group", "hello", "olleh", Data { visible: false })
    ///     .test("group", "world", "dlrow", Data { visible: true })
    ///     .run_command(["node", "solution.js"])
    ///     .build();
    /// let context = Arc::new(context);
    ///
    /// let compiled = context
    ///     .test_runner(&"group")
    ///     .unwrap()
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

        let test_count = tests.len();
        TestHandle {
            joinset: tests,
            test_count,
            compile_result: self.compile_result,
            cwd: self.cwd,
            _tmpdir: self.tmpdir,
        }
    }
}

/// The state of a test result
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
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
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
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

    /// Take the data associated with this test.  After the first call, will always return [`None`].
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
    cwd: PathBuf,
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

    /// Get the directory in which the tests are being run
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

impl<T: 'static> TestHandle<T> {
    /// Wait for the next test to finish.  The test result returned from this is _not_ ordered, but
    /// the index may be received from [`TestResult::index`].
    ///
    /// # Cancel Safety
    ///
    /// This method is cancel safe. If `wait_next` is used as the event in a `tokio::select!`
    /// statement and some other branch completes first, it is guaranteed that no test results
    /// were removed from this `TestHandle`.
    pub async fn wait_next(&mut self) -> Result<Option<TestResult<T>>, SpawnTestError> {
        match self.joinset.join_next().await {
            Some(Err(e)) => Err(SpawnTestError::JoinError(e)),
            Some(Ok(v)) => Ok(Some(v?)),
            None => Ok(None),
        }
    }

    /// Wait for all tests to complete and return a vector of test cases.  The returned vector _is
    /// ordered_ based on the order in which the tests were inserted in the [`TestContext`].
    ///
    /// # Cancel Safety
    ///
    /// This method is not cancellation safe. If the method is used as the event in a
    /// `tokio::select!` statement and some other branch completes first, then some test results
    /// may already have been consumed.
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

#[cfg(test)]
mod test {
    use std::{io::Cursor, path::Path, sync::Arc, time::Duration};

    use leucite::Rules;
    use tmpdir::TmpDir;

    use crate::{
        runner::{BorrowedFileConfig, BorrowedFileContent, TestResult, TestResultState},
        Bytes, Output, TestContext,
    };

    #[tokio::test]
    async fn test_file_config_path() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();
        let input = tmpdir.as_ref().join("in.rs");
        tokio::fs::write(&input, "some content from string")
            .await
            .expect("failed setting up test");

        let mut config = BorrowedFileConfig::new(&input, Path::new("out.rs"));
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content from string");
    }

    #[tokio::test]
    async fn test_file_config_bytes() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();

        let mut config =
            BorrowedFileConfig::new(&b"some content from bytes"[..], Path::new("out.rs"));
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content from bytes");
    }

    #[tokio::test]
    async fn test_file_config_reader() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();

        let inner = b"some content from reader".to_vec();
        let mut reader = Cursor::new(inner);

        let mut config = BorrowedFileConfig::new(
            BorrowedFileContent::reader(&mut reader),
            Path::new("out.rs"),
        );
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content from reader");
        assert_eq!(reader.position(), reader.into_inner().len() as _);
    }

    #[test]
    fn test_file_config_from_tuple2() {
        let cfg: BorrowedFileConfig = (Path::new("foo/bar"), Path::new("foo/bar")).into();
        assert_eq!(
            cfg,
            BorrowedFileConfig::new(Path::new("foo/bar"), Path::new("foo/bar"))
        );
    }

    #[test]
    fn file_content_path() {
        let content = BorrowedFileContent::path(Path::new("foo/bar"));
        assert_eq!(content, BorrowedFileContent::from(Path::new("foo/bar")));
        let content: BorrowedFileContent = Path::new("foo/bar").into();
        assert_eq!(content, BorrowedFileContent::from(Path::new("foo/bar")));
    }

    #[test]
    fn test_file_content_string() {
        let content = BorrowedFileContent::string("hello world");
        assert_eq!(content, BorrowedFileContent::from("hello world".as_bytes()));
    }

    #[test]
    fn test_file_content_bytes() {
        let bytes = vec![0xca, 0xfe, 0xba, 0xbe];
        let content = BorrowedFileContent::bytes(&bytes);
        assert_eq!(content, BorrowedFileContent::from(&*bytes));
    }

    #[test]
    fn test_file_content_equality() {
        let path1 = BorrowedFileContent::path(Path::new("a"));
        let path2 = BorrowedFileContent::path(Path::new("b"));
        let bytes1 = BorrowedFileContent::bytes(b"a");
        let bytes2 = BorrowedFileContent::bytes(b"b");
        let mut a_bytes = &b"a"[..];
        let mut b_bytes = &b"b"[..];
        let reader1 = BorrowedFileContent::reader(&mut a_bytes);
        let reader2 = BorrowedFileContent::reader(&mut b_bytes);

        assert_eq!(path1, path1);
        assert_ne!(path1, path2);

        assert_ne!(path1, bytes1);
        assert_ne!(bytes1, path1);

        assert_ne!(path1, reader1);
        assert_ne!(reader1, path1);

        assert_eq!(bytes1, bytes1);
        assert_ne!(bytes1, bytes2);

        assert_ne!(bytes1, reader1);
        assert_ne!(reader1, bytes1);

        assert_ne!(reader1, reader2);
        assert_ne!(reader2, reader1);
    }

    #[test]
    fn absolute_file_destination() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);
        let runner = ctx
            .test_runner(&0)
            .unwrap()
            .file(&b"hi"[..], Path::new("/bar.txt"));
        assert_eq!(runner.files[0].dest(), Path::new("bar.txt"));
    }

    #[test]
    fn absolute_files_destination() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(1, "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);
        let runner = ctx.test_runner(&1).unwrap().files([
            (&b"hello"[..], Path::new("/bar.txt")),
            (&b"world"[..], Path::new("/foo/bar.rs")),
        ]);
        assert_eq!(runner.files[0].dest(), Path::new("bar.txt"));
        assert_eq!(runner.files[1].dest(), Path::new("foo/bar.rs"));
    }

    #[test]
    fn test_groups_equal() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "foo", "bar", ())
            .test(0, "bar", "baz", ())
            .test(0, "baz", "qux", ())
            .test(1, "1", "2", ())
            .test(1, "2", "3", ())
            .test(1, "3", "4", ())
            .build();
        let ctx = Arc::new(ctx);
        let runner1 = Arc::clone(&ctx).test_runner(&0).unwrap();
        let runner2 = Arc::clone(&ctx).test_runner(&0).unwrap();
        assert_eq!(runner1.test_cases, runner2.test_cases);
    }

    #[test]
    fn test_groups_test_groups_equal() {
        let ctx1 = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "foo", "bar", ())
            .test(0, "bar", "baz", ())
            .test(0, "baz", "qux", ())
            .test(0, "qux", "quux", ())
            .test(1, "1", "2", ())
            .test(1, "2", "3", ())
            .test(1, "3", "4", ())
            .test(1, "4", "5", ())
            .build();

        let ctx2 = TestContext::builder()
            .run_command(["echo", "foo"])
            .test_groups([
                (
                    0,
                    [
                        ("foo", "bar", ()),
                        ("bar", "baz", ()),
                        ("baz", "qux", ()),
                        ("qux", "quux", ()),
                    ],
                ),
                (
                    1,
                    [
                        ("1", "2", ()),
                        ("2", "3", ()),
                        ("3", "4", ()),
                        ("4", "5", ()),
                    ],
                ),
            ])
            .build();

        assert_eq!(ctx1.test_cases, ctx2.test_cases);
    }

    #[test]
    fn test_groups_different() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "foo", "bar", ())
            .test(0, "bar", "baz", ())
            .test(0, "baz", "qux", ())
            .test(1, "1", "2", ())
            .test(1, "2", "3", ())
            .test(1, "3", "4", ())
            .build();

        let ctx = Arc::new(ctx);
        let runner0 = Arc::clone(&ctx).test_runner(&0).unwrap();
        let runner1 = Arc::clone(&ctx).test_runner(&1).unwrap();
        assert_ne!(runner0.test_cases, runner1.test_cases);
    }

    #[test]
    fn runner_test_tests_equivalent() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let runner1 = Arc::clone(&ctx).test_runner(&0).unwrap().files([
            (&b"hello"[..], Path::new("/bar.txt")),
            (&b"world"[..], Path::new("/foo/bar.rs")),
        ]);

        let runner2 = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .file(&b"hello"[..], Path::new("/bar.txt"))
            .file(&b"world"[..], Path::new("/foo/bar.rs"));

        assert_eq!(runner1.files, runner2.files);
    }

    #[tokio::test]
    async fn runner_with_filter() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", false)
            .test(0, "hello", "world", false)
            .test(0, "hello", "world", false)
            .build();
        let ctx = Arc::new(ctx);

        let handle = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .filter_tests(|t| *t.data())
            .compile_and_run()
            .await
            .unwrap();

        assert_eq!(handle.test_count(), 3);
    }

    #[tokio::test]
    async fn runner_without_filter() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", true)
            .test(0, "hello", "world", false)
            .test(0, "hello", "world", false)
            .test(0, "hello", "world", false)
            .build();
        let ctx = Arc::new(ctx);

        let handle = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .compile_and_run()
            .await
            .unwrap();

        assert_eq!(handle.test_count(), 6);
    }

    #[tokio::test]
    async fn custom_cwd() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", ())
            .file(b"some content", "foo.txt")
            .build();
        let ctx = Arc::new(ctx);

        let tmpdir = TmpDir::new("erudite-test").await.expect("creating tmpdir");
        let _runner = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .cwd(tmpdir.as_ref())
            .compile()
            .await
            .unwrap();

        let s = tokio::fs::read_to_string(dbg!(tmpdir.as_ref().join("foo.txt")))
            .await
            .expect("reading tmpdir");

        assert_eq!(s, "some content");
    }

    #[tokio::test]
    async fn create_file_error_missing_target() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", ())
            .file(b"some content", "foo.txt")
            .build();
        let ctx = Arc::new(ctx);

        let ret = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .cwd(Path::new("/path/does/not/exist"))
            .compile()
            .await;
        assert!(ret.is_err());
    }

    #[tokio::test]
    async fn create_file_error_missing_source_context() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", ())
            .file(Path::new("/path/does/not/exist"), "foo.txt")
            .build();
        let ctx = Arc::new(ctx);

        let ret = Arc::clone(&ctx).test_runner(&0).unwrap().compile().await;
        assert!(ret.is_err());
    }

    #[tokio::test]
    async fn create_file_error_missing_source_runner() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test(0, "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let ret = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .file(Path::new("/path/does/not/exist"), Path::new("foo.txt"))
            .compile()
            .await;
        assert!(ret.is_err());
    }

    #[tokio::test]
    async fn no_collect_output() {
        let ctx = TestContext::builder()
            .compile_command(["echo", "hello"])
            .run_command(["echo", "world"])
            .test(0, "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let output = Arc::clone(&ctx)
            .test_runner(&0)
            .unwrap()
            .collect_output(false)
            .compile()
            .await
            .unwrap();

        assert!(output.compile_result().unwrap().stdout().is_empty());
        assert!(output.compile_result().unwrap().stderr().is_empty());
        assert_eq!(output.compile_result().unwrap().exit_status(), 0);
    }

    #[tokio::test]
    async fn compile_timeout() {
        let ctx = TestContext::builder()
            .compile_command(["sleep", "10s"])
            .run_command(["echo", "world"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx)
            .test_runner(&())
            .unwrap()
            .collect_output(false)
            .compile()
            .await;

        assert!(res.is_err());
    }

    #[tokio::test]
    async fn compile_only_rules() {
        let rules = Rules::new();
        let ctx = TestContext::builder()
            .compile_command(["cat", "/bin/cat"])
            .run_command(["echo", "world"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .compile_rules(rules)
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx).default_test_runner().compile().await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn run_only_rules() {
        let rules = Rules::new();
        let ctx = TestContext::builder()
            .compile_command(["echo", "hello"])
            .run_command(["cat", "/bin/cat"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .run_rules(rules)
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx)
            .default_test_runner()
            .compile()
            .await
            .unwrap();
        let mut tests = res.run();
        assert!(tests.wait_next().await.is_err());
    }

    #[tokio::test]
    async fn both_rules_unique_compile_pass() {
        let ctx = TestContext::builder()
            .compile_command(["echo", "/bin/cat"])
            .run_command(["cat", "/bin/cat"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .compile_rules(Rules::new().add_read_only("/usr").add_read_only("/bin"))
            .run_rules(Rules::new())
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx)
            .default_test_runner()
            .compile()
            .await
            .unwrap();
        let mut tests = res.run();
        assert!(tests.wait_next().await.is_err());
    }

    #[tokio::test]
    async fn both_rules_unique_compile_fail() {
        let ctx = TestContext::builder()
            .compile_command(["echo", "/bin/cat"])
            .run_command(["cat", "/bin/cat"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .compile_rules(Rules::new())
            .run_rules(Rules::new().add_read_only("/usr").add_read_only("/bin"))
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx).default_test_runner().compile().await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn both_rules_same() {
        let ctx = TestContext::builder()
            .compile_command(["echo", "/bin/cat"])
            .run_command(["cat", "/bin/cat"])
            .compile_timeout(Duration::from_millis(100))
            .test((), "hello", "world", ())
            .rules(Rules::new().add_read_only("/usr").add_read_only("/bin"))
            .build();
        let ctx = Arc::new(ctx);

        let res = Arc::clone(&ctx)
            .default_test_runner()
            .compile()
            .await
            .unwrap();
        let mut tests = res.run();
        let test0 = tests.wait_next().await.unwrap().unwrap();
        assert_eq!(test0.state(), TestResultState::IncorrectOutput); // /bin/cat is almost certainly not "world"
    }

    #[tokio::test]
    async fn test_result_getters() {
        let mut result = TestResult {
            index: 42,
            data: Some(String::from("this is not copy")),
            output: Output::new("stdout".to_string(), "stderr".to_string(), 69),
            state: TestResultState::Pass,
            time_taken: Duration::from_secs(2),
        };

        assert_eq!(result.data(), Some(&"this is not copy".to_string()));
        assert_eq!(result.data(), Some(&"this is not copy".to_string()));
        assert_eq!(result.take_data(), Some("this is not copy".to_string()));
        assert_eq!(result.take_data(), None);
        assert_eq!(result.data(), None);

        assert_eq!(result.index(), 42);
        assert_eq!(
            result.output(),
            &Output::new("stdout".to_string(), "stderr".to_string(), 69)
        );
        assert_eq!(result.stdout(), &Bytes::from("stdout".to_string()));
        assert_eq!(result.stderr(), &Bytes::from("stderr".to_string()));
        assert_eq!(result.exit_status(), 69);
        assert_eq!(result.state(), TestResultState::Pass);
        assert_eq!(result.time_taken(), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn test_handle_getters() {
        let ctx = TestContext::builder()
            .run_command(["echo", "world"])
            .test((), "hello", "world", ())
            .test((), "hello", "world", ())
            .test((), "hello", "world", ())
            .test((), "hello", "world", ())
            .test((), "hello", "world", ())
            .test((), "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let mut runner = ctx.default_test_runner().compile_and_run().await.unwrap();
        assert_eq!(runner.tests_left(), 6);
        assert_eq!(runner.test_count(), 6);
        assert_eq!(runner.compile_result(), None);
        let _result = runner.wait_next().await.unwrap().unwrap();
        assert_eq!(runner.tests_left(), 5);
        assert_eq!(runner.test_count(), 6);
        assert_eq!(runner.compile_result(), None);

        let ctx = TestContext::builder()
            .compile_command(["echo", "world"])
            .run_command(["echo", "world"])
            .test((), "", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let runner = ctx.default_test_runner().compile_and_run().await.unwrap();
        assert!(runner.compile_result().is_some());
    }

    #[tokio::test]
    async fn compile_result_getters() {
        let ctx = TestContext::builder()
            .compile_command(["echo", "hello"])
            .run_command(["echo", "world"])
            .test((), "hello", "world", ())
            .build();
        let ctx = Arc::new(ctx);

        let runner = ctx.default_test_runner().compile().await.unwrap();
        let result = runner.compile_result().unwrap();
        assert_eq!(
            result.output(),
            &Output::new("hello\n".to_string(), "".to_string(), 0)
        );
        assert_eq!(result.stdout(), &Bytes::from("hello\n".to_string()));
        assert!(result.stderr().is_empty());

        let handle = runner.run();
        let result = handle.compile_result().unwrap();
        assert_eq!(result.stdout(), &Bytes::from("hello\n".to_string()));
        assert!(result.stderr().is_empty());
        dbg!(result.time_taken());
        assert!(result.time_taken() > Duration::ZERO);
    }
}
