use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use leucite::{CommandExt, Rules};
use thiserror::Error;
use tmpdir::TmpDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    task::{JoinError, JoinSet},
    time::Instant,
    try_join,
};
use tracing::{debug, debug_span, error, instrument, trace, Instrument};

use crate::{
    context::{CommandConfig, OutputValidator, TestCase, TestContext},
    Bytes, SimpleOutput,
};

pub struct TestFileConfig<'a> {
    /// This path is relative to the temporary directory created while running tests
    dest: &'a Path,
    src: TestFileContent<'a>,
}

impl TestFileConfig<'_> {
    pub async fn write_file(&mut self, base: &Path) -> std::io::Result<u64> {
        let target = base.join(self.dest);
        match self.src {
            TestFileContent::Path(ref path) => tokio::fs::copy(path, target).await,
            TestFileContent::Bytes(ref contents) => tokio::fs::write(target, contents)
                .await
                .map(|_| contents.len() as _),
            TestFileContent::Reader(ref mut reader) => {
                let mut out = tokio::fs::File::open(target).await?;
                tokio::io::copy(reader, &mut out).await
            }
        }
    }

    pub fn dest(&self) -> &Path {
        self.dest
    }
}

pub trait AsyncReadUnpin: AsyncRead + Unpin {}
impl<T> AsyncReadUnpin for T where T: AsyncRead + Unpin {}

pub enum TestFileContent<'a> {
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
    pub fn path(path: &'a Path) -> Self {
        Self::Path(path)
    }

    pub fn string(string: &'a str) -> Self {
        Self::Bytes(string.as_bytes())
    }

    pub fn bytes(bytes: &'a [u8]) -> Self {
        Self::Bytes(bytes)
    }
}

fn command_from_argv(argv: &[String]) -> Option<Command> {
    let (program, args) = argv.split_first()?;

    let mut cmd = Command::new(program);
    cmd.args(args);
    Some(cmd)
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("Failed to spawn compile command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    #[error("Failed to wait on compile command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
    #[error("Invalid compile command specified")]
    InvalidCommand,
    #[error("Failed to create compile/run directory: {:?}", .0)]
    MktempFail(#[source] std::io::Error),
}

#[derive(Debug, Error)]
#[error("Failed to create file at {}: {:?}", .path.display(), .error)]
pub struct CreateFilesError {
    path: PathBuf,
    #[source]
    error: std::io::Error,
}

#[derive(Debug, Error)]
pub enum SpawnTestError {
    #[error("Failed to join thread: {:?}", .0)]
    JoinError(JoinError),
    #[error("Invalid run command specified")]
    InvalidCommand,
    #[error("Failed to spawn run command: {:?}", .0)]
    SpawnFail(#[source] std::io::Error),
    #[error("Failed to write to stdin of test program: {:?}", .0)]
    WriteStdinFail(#[source] std::io::Error),
    #[error("Failed to wait on run command: {:?}", .0)]
    WaitFail(#[source] std::io::Error),
}

#[derive(Debug, Error)]
pub enum CompileAndSpawnError {
    #[error("failed to compile compile: {:?}", .0)]
    CompileError(#[from] CompileError),
    #[error("failed to create necessary files: {:?}", .0)]
    CreateFilesError(#[from] CreateFilesError),
    #[error("failed to spawn tests: {:?}", .0)]
    SpawnTestError(#[from] SpawnTestError),
}

async fn wait_with_output_and_timeout(
    child: &mut Child,
    timeout: Option<Duration>,
    input: Option<&str>,
) -> std::io::Result<(SimpleOutput, bool)> {
    let stdin_input = if let Some(input) = input {
        Some((child.stdin.take().expect("We only take this once"), input))
    } else {
        None
    };
    let stdout_pipe = child.stdout.take().expect("We only take this once");
    let stderr_pipe = child.stderr.take().expect("We only take this once");

    async fn read_to_end<R>(mut r: R) -> std::io::Result<Vec<u8>>
    where
        R: AsyncRead + Unpin,
    {
        let mut out = Vec::new();
        let bytes = r.read_to_end(&mut out).await?;
        trace!(bytes, "finished reading from stdout");
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
        w.write_u8(b'\n') // Required for empty input to work well in some languages
            .await?;
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
    let stdout_fut = read_to_end(stdout_pipe);
    let stderr_fut = read_to_end(stderr_pipe);
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

    Ok((SimpleOutput::new(stdout, stderr, exit_status), timed_out))
}

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
    pub fn new(context: Arc<TestContext<T>>) -> Self {
        Self {
            context,
            files: Default::default(),
            test_filter: None,
            cwd: None,
            collect_output: true,
        }
    }

    // Note: the `run` method consumes `self`, so everything else should, too.
    pub fn file(mut self, source: impl Into<TestFileContent<'a>>, dest: &'a Path) -> Self {
        self.files.push(TestFileConfig {
            dest,
            src: source.into(),
        });
        self
    }

    pub fn filter_tests(mut self, filter: fn(&TestCase<T>) -> bool) -> Self {
        self.test_filter = Some(filter);
        self
    }

    pub fn cwd(mut self, cwd: &'a Path) -> Self {
        self.cwd = Some(cwd);
        self
    }

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

    /// # Return values:
    ///
    /// Ok(Some(_)) - Command ran successfully
    /// Ok(None)    - Command did not need to run
    /// Err(_)      - Error while spawning/running compile command
    /// If `collect_output` is false, only the `status` field on the returned result (ok or err) is
    /// set to the correct value.
    async fn compile(
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
            CompileResultState::Pass
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
        } else if let Some(stdout) = output.stdout.str() {
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

    fn spawn_tests(
        &mut self,
        cwd: &Path,
        run_rules: Option<Arc<Rules>>,
    ) -> JoinSet<Result<TestResult<T>, SpawnTestError>> {
        let mut joinset = JoinSet::new();

        for (i, case) in self.context.test_cases.iter().enumerate() {
            if self.test_filter.is_some_and(|filter| !filter(case)) {
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
            let context = Arc::clone(&self.context);
            let run_rules = run_rules.as_ref().map(Arc::clone);
            let path = cwd.to_path_buf();
            joinset.spawn(
                async move { Self::run_test(i, &path, case, run_rules, context).await }
                    .instrument(span),
            );
        }

        joinset
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

    #[instrument(skip(self))]
    pub async fn compile_and_spawn_runner(
        mut self,
    ) -> Result<(Option<CompileResult>, TestHandle<T>), CompileAndSpawnError> {
        let mut tmpdir = None;
        let cwd = if let Some(cwd) = self.cwd.take() {
            trace!(?cwd, "Using specified cwd");
            cwd
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
                .as_ref()
        };

        debug!(path = ?cwd, "setting up directory");

        let (compile_rules, run_rules) = self.create_rules(cwd);

        trace!("creating files");
        let start = Instant::now();
        self.create_files(cwd).await?;
        let elapsed = start.elapsed();
        debug!(in = ?elapsed, "created files");

        debug!(?cwd, "starting compilation");
        let start = Instant::now();
        let compile_output = self.compile(cwd, compile_rules).await?;
        let elapsed = start.elapsed();
        debug!(in = ?elapsed, "finished compilation");

        trace!("spawning tests");
        let tests = self.spawn_tests(cwd, run_rules);

        Ok((
            compile_output,
            TestHandle {
                joinset: tests,
                _tmpdir: tmpdir,
            },
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TestResultState {
    Pass,
    RuntimeFail,
    TimedOut,
    IncorrectOutput,
}

#[derive(Debug)]
pub struct TestResult<T> {
    index: usize,
    /// Option so that it can be taken using [`Self::take_data`]
    data: Option<T>,
    output: SimpleOutput,
    state: TestResultState,
    time_taken: Duration,
}

impl<T> TestResult<T> {
    pub fn index(&self) -> usize {
        self.index
    }

    /// Take the data associated with this test.  After the first call, will always return [`None`]..
    pub fn take_data(&mut self) -> Option<T> {
        self.data.take()
    }

    /// Get the data associated with this test.  Returns `None` if the data has been taken via
    /// [`Self::take_data`].
    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    pub fn time_taken(&self) -> Duration {
        self.time_taken
    }

    pub fn state(&self) -> TestResultState {
        self.state
    }

    pub fn output(&self) -> &SimpleOutput {
        &self.output
    }

    pub fn stdout(&self) -> &Bytes {
        &self.output.stdout
    }

    pub fn stderr(&self) -> &Bytes {
        &self.output.stderr
    }

    pub fn exit_status(&self) -> i32 {
        self.output.status
    }
}

pub struct TestHandle<T> {
    joinset: JoinSet<Result<TestResult<T>, SpawnTestError>>,
    /// Needed to remove the temp dir when we're done
    _tmpdir: Option<TmpDir>,
}

impl<T: 'static> TestHandle<T> {
    pub fn len(&self) -> usize {
        self.joinset.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joinset.is_empty()
    }

    pub async fn wait_next(&mut self) -> Result<Option<TestResult<T>>, SpawnTestError> {
        match self.joinset.join_next().await {
            Some(Err(e)) => Err(SpawnTestError::JoinError(e)),
            Some(Ok(v)) => Ok(Some(v?)),
            None => Ok(None),
        }
    }

    pub async fn wait_all(&mut self) -> Result<Vec<TestResult<T>>, SpawnTestError> {
        let len = self.len();
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompileResultState {
    Pass,
    RuntimeFail,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileResult {
    output: SimpleOutput,
    state: CompileResultState,
    time_taken: Duration,
}

impl CompileResult {
    pub fn time_taken(&self) -> Duration {
        self.time_taken
    }

    pub fn state(&self) -> CompileResultState {
        self.state
    }

    pub fn output(&self) -> &SimpleOutput {
        &self.output
    }

    pub fn stdout(&self) -> &Bytes {
        &self.output.stdout
    }

    pub fn stderr(&self) -> &Bytes {
        &self.output.stderr
    }

    pub fn exit_status(&self) -> i32 {
        self.output.status
    }
}
