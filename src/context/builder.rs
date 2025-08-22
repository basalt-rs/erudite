use crate::cases::{ExpectedOutput, TestCase};

use super::{CommandConfig, FileConfig, FileContent, TestContext};
use std::{marker::PhantomData, path::PathBuf, time::Duration};

use leucite::{MemorySize, Rules};

macro_rules! define_state_structs {
    ($($unset: ident => $set: ident),+$(,)?) => {
        $(define_state_structs!(@ $unset, $set);)+
    };
    (@ $($name: ident),+$(,)?) => {
        $(
            #[non_exhaustive]
            #[doc(hidden)]
            pub struct $name;
        )+
    };
}

// NOTE: this is hidden to reduce potential misuse
pub(crate) mod hidden {
    define_state_structs! {
        MissingTests => SetTests,
        MissingRunCmd => SetRunCmd,
    }
}
use hidden::*;

/// A builder that will crate a [`TestContext`].  This builder uses a type-state pattern to ensure
/// that the require fields are all present.
///
/// The fields `timeout`, `rules`, `max_memory`, `max_file_size`, and `max_threads` can be set for
/// runtime, compile-time, or both.  These methods have a pattern of `run_<field>` for runtime
/// only, `compile_<field>` for compile-time only, or `<field>` for both runtime and compile-time.
///
/// # Usage
///
/// ```
/// # use erudite::{context::{FileContent, TestContext}, Rules, MemorySize};
/// # use std::time::Duration;
/// # let rules = Rules::new();
/// let context = TestContext::builder()
///     .compile_command(["rustc", "-o", "main", "main.rs"])
///     .run_command(["./main"])   // Run command is always required
///     .test("hello", "olleh", 1) // At least one test is required
///     .test("world", "dlrow", 2)
///     .test("rust", "tsur", 3)
///     .test("tacocat", "tacocat", 4)
///     .trim_output(true)
///     .file(FileContent::string("// some rust code"), "main.rs")
///     .timeout(Duration::from_secs(5))        // or `run_timeout`/`compile_timeout`
///     .rules(rules)                           // or `run_rules`/`compile_rules`
///     .max_memory(MemorySize::from_gib(1))    // or `run_max_memory`/`compile_max_memory`
///     .max_file_size(MemorySize::from_gib(1)) // or `run_max_file_size`/`compile_max_file_size`
///     .max_threads(2)                         // or `run_max_threads`/`compile_max_threads`
///     .build();
/// ```
#[derive(Debug, Clone)]
#[must_use]
pub struct TestContextBuilder<T, Tests = MissingTests, RunCmd = MissingRunCmd> {
    command: CommandConfig<Vec<String>>, // Compile: optional, Run: required
    test_cases: Vec<TestCase<T>>,        // required (at least once)
    trim_output: bool,                   // optional
    files: Vec<FileConfig>,              // optional
    timeout: CommandConfig<Duration>,    // optional
    rules: CommandConfig<Rules>,         // optional
    max_memory: CommandConfig<MemorySize>, // optional
    max_file_size: CommandConfig<MemorySize>, // optional
    max_threads: CommandConfig<u64>,     // optional

    state: PhantomData<(Tests, RunCmd)>,
}

impl<T> TestContextBuilder<T, MissingTests, MissingRunCmd> {
    // Construction of this type should only be done with [`TestContext::builder`].
    pub(crate) fn new() -> Self {
        Self {
            trim_output: false,
            test_cases: Vec::new(),
            files: Vec::new(),
            command: Default::default(),
            timeout: Default::default(),
            rules: Default::default(),
            max_memory: Default::default(),
            max_file_size: Default::default(),
            max_threads: Default::default(),
            state: PhantomData,
        }
    }
}

impl<T, A, B> TestContextBuilder<T, A, B> {
    /// Convert a TestContextBuilder<T, A, B> into TestContextBuilder<T, C, D>
    // NOTE: This function _must not_ be made public in any way, or the type-state builder can be
    // invalidated.
    fn transform<C, D>(self) -> TestContextBuilder<T, C, D> {
        TestContextBuilder {
            test_cases: self.test_cases,
            trim_output: self.trim_output,
            files: self.files,
            command: self.command,
            timeout: self.timeout,
            rules: self.rules,
            max_memory: self.max_memory,
            max_file_size: self.max_file_size,
            max_threads: self.max_threads,
            state: PhantomData,
        }
    }
}

macro_rules! command_config_fns {
    ($field: ident, $type: ty, $noun: literal) => {
        concat_idents::concat_idents!(ident = run_, $field {
            #[doc = concat!("Set the ", $noun, " when running the program")]
            pub fn ident(mut self, $field: $type) -> Self {
                self.$field.with_run($field);
                self
            }
        });
        concat_idents::concat_idents!(ident = compile_, $field {
            #[doc = concat!("Set the ", $noun, " when compiling the program")]
            pub fn ident(mut self, $field: $type) -> Self {
                self.$field.with_compile($field);
                self
            }
        });

        #[doc = concat!("Set the ", $noun, " of _both_ running and compiling the program")]
        pub fn $field(mut self, $field: $type) -> Self {
            self.$field.with_both($field);
            self
        }
    };
}

// Optional fields
impl<T, Tests, RunCmd> TestContextBuilder<T, Tests, RunCmd> {
    /// Set whether the tests should trim the output of the program before testing (using
    /// [`str::trim`])
    ///
    /// Default = `false`
    ///
    /// ```
    /// # use erudite::context::TestContext;
    /// let ctx = TestContext::builder()
    ///     .compile_command(["gcc", "-o", "solution", "solution.c"])
    ///     .run_command(["solution.c"])
    ///     .test("hello world", "dlrow olleh", true)
    ///     .trim_output(true)
    ///     .build();
    /// ```
    pub fn trim_output(mut self, trim_output: bool) -> Self {
        self.trim_output = trim_output;
        self
    }

    /// Set the command used for compilation of the test.  If this function is not called, then
    /// _no_ compilation step will be performed, even if the other compile settings are used.
    ///
    /// The command must have at least one item to be valid.  If it does not, the error will be
    /// propogated when calling [`TestRunner::compile`]
    ///
    /// [`TestRunner::compile`]: crate::runner::TestRunner::compile
    ///
    /// ```
    /// # use erudite::context::{TestContext, FileContent};
    /// let ctx = TestContext::builder()
    ///     .compile_command(["gcc", "-o", "solution", "solution.c"])
    ///     .run_command(["solution.c"])
    ///     .test("hello world", "dlrow olleh", true)
    ///     .build();
    /// ```
    pub fn compile_command(
        mut self,
        compile_command: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.command
            .with_compile(compile_command.into_iter().map(Into::into).collect());
        self
    }

    /// Add a file to be inserted into the runtime environment of the test.  This gets added
    /// before compilation, so it works well for libraries or input/output manipulation.
    ///
    /// If `source` is a path, the file will only be copied when a test is compiled, meaning
    /// that if the file changes or is removed, then the output may differ between two
    /// instances of the test runner from a single context.
    ///
    /// If the intended behaviour is to read the file _now_, consider reading the file directly
    /// and placing it into [`FileContent::bytes`].
    ///
    /// `destination` is path relative to the directory used for the compilation/runtime
    /// environment.  If destination is not a relative path, _this function will panic_.
    ///
    /// ```
    /// # use erudite::context::{TestContext, FileContent};
    /// let ctx = TestContext::builder()
    ///     .run_command(["node", "solution.js"])
    ///     .test("hello world", "dlrow olleh", true)
    ///     .file(FileContent::string("// some javascript code"), "solution.js")
    ///     .file(FileContent::path("/foo/bar"), "some_other_file")
    ///     .build();
    /// ```
    pub fn file(mut self, source: impl Into<FileContent>, destination: impl Into<PathBuf>) -> Self {
        let destination = destination.into();

        assert!(
            destination.is_relative(),
            "Destination is not a relative path (destination = {})",
            destination.display(),
        );

        self.files.push(FileConfig {
            dest: destination,
            src: source.into(),
        });

        self
    }

    command_config_fns!(rules, Rules, "rules for execution");
    command_config_fns!(max_memory, MemorySize, "maximum memory usage");
    command_config_fns!(max_file_size, MemorySize, "maximum file size");
    command_config_fns!(max_threads, u64, "max thread count");
    command_config_fns!(timeout, Duration, "timeout");
}

// `.run_command` when no command has been added
impl<T, Tests> TestContextBuilder<T, Tests, MissingRunCmd> {
    /// Set the command to run the tests
    ///
    /// The command must have at least one item to be valid.  If it does not, the error will be
    /// propogated when calling [`TestHandle::wait_next`]
    ///
    /// [`TestHandle::wait_next`]: crate::runner::TestHandle::wait_next
    ///
    /// ```
    /// # use erudite::context::TestContext;
    /// let ctx = TestContext::builder()
    ///     .run_command(["node", "solution.js"])
    ///     .test("hello world", "dlrow olleh", true)
    ///     .build();
    /// ```
    pub fn run_command(
        mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> TestContextBuilder<T, Tests, SetRunCmd> {
        self.command
            .with_run(command.into_iter().map(Into::into).collect());
        self.transform()
    }
}

impl<T, Tests, RunCmd> TestContextBuilder<T, Tests, RunCmd>
where
    T: Clone, // This bound is a little bit arbitrary here, but it makes errors cleaner as
              // [`TestContext::test_builder`] needs it.
{
    /// Add a single test to this context
    ///
    /// The `data` argument is for any data that you wish to associate with this test (on top of
    /// its index, which is assocated by default).  This is useful for adding additional
    /// information such as visiblity to the test.
    ///
    /// The data is accessible by [`TestRunner::filter_tests`] and [`TestResult::data`].
    ///
    /// [`TestRunner::filter_tests`]: crate::runner::TestRunner::filter_tests
    /// [`TestResult::data`]: crate::runner::TestResult::data
    ///
    /// ```
    /// # use erudite::context::TestContext;
    /// let ctx: TestContext<bool> = TestContext::builder()
    ///     .run_command(["echo", "hi"])
    ///     .test("hello world", "dlrow olleh", true)
    ///     .build();
    /// ```
    pub fn test(
        mut self,
        input: impl Into<String>,
        output: impl Into<ExpectedOutput>,
        data: T,
    ) -> TestContextBuilder<T, SetTests, RunCmd> {
        self.test_cases.push(TestCase::new(input, output, data));
        self.transform()
    }

    /// Add a series of tests to this context
    ///
    /// See [`Self::test`] for additional information.
    ///
    /// ```
    /// # use erudite::context::TestContext;
    /// let ctx: TestContext<bool> = TestContext::builder()
    ///     .run_command(["echo", "hi"])
    ///     .tests([
    ///         ("hello world", "dlrow olleh", true),
    ///         ("good morning", "gninrom doog", true),
    ///     ])
    ///     .build();
    /// ```
    // NOTE: this function in the type-state builder is not perfect, as they could pass an iterator
    // with 0 elements, but it's a relatively rare case.
    pub fn tests(
        mut self,
        tests: impl IntoIterator<Item = impl Into<TestCase<T>>>,
    ) -> TestContextBuilder<T, SetTests, RunCmd> {
        self.test_cases.extend(tests.into_iter().map(Into::into));
        assert!(!self.test_cases.is_empty()); // This will catch an empty iterator, but it's inelegant.
        self.transform()
    }
}

impl<T> TestContextBuilder<T, SetTests, SetRunCmd> {
    /// Finish building this test context
    pub fn build(self) -> TestContext<T> {
        // Just some sanity checks
        debug_assert!(self.command.run().is_some());
        debug_assert!(!self.test_cases.is_empty());
        TestContext {
            trim_output: self.trim_output,
            files: self.files,
            test_cases: self.test_cases,
            command: self.command.into(),
            timeout: self.timeout,
            rules: self.rules,
            max_memory: self.max_memory,
            max_file_size: self.max_file_size,
            max_threads: self.max_threads,
        }
    }
}

/// A few `compile_fail` tests to confirm the efficacy of the type-state builder
///
/// No run method or tests
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .build();
/// ```
///
/// One test, but no run method
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .test("foo", "bar", ())
///     .build();
/// ```
///
/// Many tests, but no run method
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .test("foo", "bar", ())
///     .test("foo", "bar", ())
///     .test("foo", "bar", ())
///     .build();
/// ```
///
/// One test using `.tests`, but no run method
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .tests([("foo", "bar", ())])
///     .build();
/// ```
///
/// Many tests using `.tests`, but no run method
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .tests([("foo", "bar", ()), ("baz", "qux", ())])
///     .build();
/// ```
///
/// Run method, but no tests
/// ```compile_fail
/// use erudite::context::TestContext;
/// let context = TestContext::builder()
///     .run_command(["rustc", "-o", "solution", "solution.rs"])
///     .build();
/// ```
#[allow(unused)]
#[doc(hidden)]
fn type_state_builder_test() {}
