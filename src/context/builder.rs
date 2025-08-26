use crate::{
    cases::{ExpectedOutput, TestCase},
    FileContent,
};

use super::{FileConfig, StageConfig, TestContext};
use std::{collections::HashMap, hash::Hash, marker::PhantomData, path::Path, time::Duration};

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
/// # use erudite::{TestContext, FileContent, Rules, MemorySize};
/// # use std::time::Duration;
/// # let rules = Rules::new();
/// let context = TestContext::builder()
///     .compile_command(["rustc", "-o", "main", "main.rs"])
///     .run_command(["./main"])   // Run command is always required
///     .test("group", "hello", "olleh", 1) // At least one test is required
///     .test("group", "world", "dlrow", 2)
///     .test("group", "rust", "tsur", 3)
///     .test("group", "tacocat", "tacocat", 4)
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
pub struct TestContextBuilder<G, T, Tests = MissingTests, RunCmd = MissingRunCmd> {
    command: StageConfig<Vec<String>>, // Compile: optional, Run: required
    test_cases: HashMap<G, Vec<TestCase<T>>>, // required (at least once)
    trim_output: bool,                 // optional
    files: Vec<FileConfig>,            // optional
    timeout: StageConfig<Duration>,    // optional
    rules: StageConfig<Rules>,         // optional
    max_memory: StageConfig<MemorySize>, // optional
    max_file_size: StageConfig<MemorySize>, // optional
    max_threads: StageConfig<u64>,     // optional

    state: PhantomData<(Tests, RunCmd)>,
}

impl<G, T> TestContextBuilder<G, T, MissingTests, MissingRunCmd> {
    // Construction of this type should only be done with [`TestContext::builder`].
    pub(crate) fn new() -> Self {
        Self {
            trim_output: false,
            test_cases: Default::default(),
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

impl<G, T, A, B> TestContextBuilder<G, T, A, B> {
    /// Convert a TestContextBuilder<G, T, A, B> into TestContextBuilder<G, T, C, D>
    // NOTE: This function _must not_ be made public in any way, or the type-state builder can be
    // invalidated.
    fn transform<C, D>(self) -> TestContextBuilder<G, T, C, D> {
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
impl<G, T, Tests, RunCmd> TestContextBuilder<G, T, Tests, RunCmd> {
    /// Set whether the tests should trim the output of the program before testing (using
    /// [`str::trim`])
    ///
    /// Default = `false`
    ///
    /// ```
    /// # use erudite::TestContext;
    /// let ctx = TestContext::builder()
    ///     .compile_command(["gcc", "-o", "solution", "solution.c"])
    ///     .run_command(["solution.c"])
    ///     .test("group", "hello world", "dlrow olleh", true)
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
    /// # use erudite::{FileContent, TestContext};
    /// let ctx = TestContext::builder()
    ///     .compile_command(["gcc", "-o", "solution", "solution.c"])
    ///     .run_command(["solution.c"])
    ///     .test("group", "hello world", "dlrow olleh", true)
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

    /// Add a file to be inserted into the runtime environment of the test.  This is added before
    /// compilation, so it works well for libraries or input/output manipulation.
    ///
    /// If `source` is a path, the file will only be copied when a test is compiled, meaning
    /// that if the file changes or is removed, then the output may differ between two
    /// instances of the test runner from a single context.
    ///
    /// If the intended behaviour is to read the file _now_, consider reading the file directly
    /// and placing it into [`FileContent::bytes`].
    ///
    /// `destination` is path relative to the directory used for the test environment.  If
    /// destination an absolute path, then it will be made relative to the test environment, i.e.,
    /// `/foo/bar` -> `<test-env>/foo/bar`.
    ///
    /// ```
    /// # use erudite::{FileContent, TestContext};
    /// let ctx = TestContext::builder()
    ///     .run_command(["node", "solution.js"])
    ///     .test("group", "hello world", "dlrow olleh", true)
    ///     .file(FileContent::string("// some javascript code"), "solution.js")
    ///     .file(FileContent::path("/foo/bar"), "some_other_file")
    ///     .build();
    /// ```
    pub fn file(mut self, source: impl Into<FileContent>, destination: impl AsRef<Path>) -> Self {
        self.files.push(FileConfig::new(source, destination));
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
    /// and placing it into [`FileContent::bytes`].
    ///
    /// `destination` is path relative to the directory used for the test environment.  If
    /// destination an absolute path, then it will be made relative to the test environment, i.e.,
    /// `/foo/bar` -> `<test-env>/foo/bar`.
    ///
    /// ```
    /// # use erudite::{FileContent, FileConfig, TestContext};
    /// let ctx = TestContext::builder()
    ///     .run_command(["node", "solution.js"])
    ///     .test("group", "hello world", "dlrow olleh", true)
    ///     .files([
    ///         FileConfig::new(FileContent::string("// some javascript code"), "solution.js"),
    ///         FileConfig::new(FileContent::path("/foo/bar"), "some_other_file")
    ///     ])
    ///     .build();
    /// ```
    pub fn files(mut self, files: impl IntoIterator<Item = impl Into<FileConfig>>) -> Self {
        self.files.extend(files.into_iter().map(Into::into));
        self
    }

    command_config_fns!(rules, Rules, "rules for execution");
    command_config_fns!(max_memory, MemorySize, "maximum memory usage");
    command_config_fns!(max_file_size, MemorySize, "maximum file size");
    command_config_fns!(max_threads, u64, "max thread count");
    command_config_fns!(timeout, Duration, "timeout");
}

// `.run_command` when no command has been added
impl<G, T, Tests> TestContextBuilder<G, T, Tests, MissingRunCmd> {
    /// Set the command to run the tests
    ///
    /// The command must have at least one item to be valid.  If it does not, the error will be
    /// propogated when calling [`TestHandle::wait_next`]
    ///
    /// [`TestHandle::wait_next`]: crate::runner::TestHandle::wait_next
    ///
    /// ```
    /// # use erudite::TestContext;
    /// let ctx = TestContext::builder()
    ///     .run_command(["node", "solution.js"])
    ///     .test("group", "hello world", "dlrow olleh", true)
    ///     .build();
    /// ```
    pub fn run_command(
        mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> TestContextBuilder<G, T, Tests, SetRunCmd> {
        self.command
            .with_run(command.into_iter().map(Into::into).collect());
        self.transform()
    }
}

impl<G, T, Tests, RunCmd> TestContextBuilder<G, T, Tests, RunCmd>
where
    T: Clone, // This bound is a little bit arbitrary here, but it makes errors cleaner as
    // [`TestContext::test_builder`] needs it.
    G: Hash + Eq,
{
    /// Add a single test to this context in a group
    ///
    /// The `data` argument is for any data that you wish to associate with this test (on top of
    /// its index, which is assocated by default).  This is useful for adding additional
    /// information such as visiblity to the test.
    ///
    /// The data is accessible by [`TestRunner::filter_tests`] and [`TestResult::data`].
    ///
    /// The `group` argument is used to associate tests with eachother.  When creating a test
    /// runner, one uses the `group` to select just the tests that they wish to run.  Group keys
    /// must implement [`Hash`] and [`Eq`], but other than that, there is no restriction.  This
    /// function requires an owned `G` for each test case, but to reduce copies,
    /// [`TestContextBuilder::tests`] may be preferred.
    ///
    /// There is a special case if `G` is `()`: a [`TestContext::default_test_runner`] becomes
    /// available, which requires no argment and does not return an option.  This is ideal if only
    /// one test group is necessary.
    ///
    /// [`TestRunner::filter_tests`]: crate::runner::TestRunner::filter_tests
    /// [`TestResult::data`]: crate::runner::TestResult::data
    ///
    /// ```
    /// # use erudite::TestContext;
    /// let ctx = TestContext::builder()
    ///     .run_command(["echo", "hi"])
    ///     .test("group", "hello world", "dlrow olleh", true)
    ///     .build();
    /// ```
    pub fn test(
        mut self,
        group: G,
        input: impl Into<String>,
        output: impl Into<ExpectedOutput>,
        data: T,
    ) -> TestContextBuilder<G, T, SetTests, RunCmd> {
        self.test_cases
            .entry(group)
            .or_default()
            .push(TestCase::new(input, output, data));
        self.transform()
    }

    /// Add a series of tests to this context
    ///
    /// The `group` argument is used to associate tests with eachother.  When creating a test
    /// runner, one uses the `group` to select just the tests that they wish to run.  Group keys
    /// must implement [`Hash`] and [`Eq`], but other than that, there is no restriction.  This
    /// function may be preferable over [`TestContextBuilder::test`] as there is only need for one
    /// allocated group key.
    ///
    /// There is a special case if `G` is `()`: a [`TestContext::default_test_runner`] becomes
    /// available, which requires no argment and does not return an option.  This is ideal if only
    /// one test group is necessary.
    ///
    /// See [`Self::test`] for additional information.
    /// ```
    /// # use erudite::TestContext;
    /// let ctx = TestContext::builder()
    ///     .run_command(["echo", "hi"])
    ///     .tests("group", [
    ///         ("hello world", "dlrow olleh", true),
    ///         ("good morning", "gninrom doog", true),
    ///     ])
    ///     .build();
    /// ```
    // NOTE: this function in the type-state builder is not perfect, as the caller could pass an
    // iterator with 0 elements, but it's a relatively rare case.
    pub fn tests(
        mut self,
        group: G,
        tests: impl IntoIterator<Item = impl Into<TestCase<T>>>,
    ) -> TestContextBuilder<G, T, SetTests, RunCmd> {
        self.test_cases
            .entry(group)
            .or_default()
            .extend(tests.into_iter().map(Into::into));
        self.transform()
    }

    /// Add several test groups to this context
    ///
    /// The `group` argument is used to associate tests with eachother.  When creating a test
    /// runner, one uses the `group` to select just the tests that they wish to run.  Group keys
    /// must implement [`Hash`] and [`Eq`], but other than that, there is no restriction.  This
    /// function may be preferable over [`TestContextBuilder::test`] as there is only need for one
    /// allocated group key.
    ///
    /// There is a special case if `G` is `()`: a [`TestContext::default_test_runner`] becomes
    /// available, which requires no argment and does not return an option.  This is ideal if only
    /// one test group is necessary.
    ///
    /// NOTE: This gets very syntactically noisy when writing out by hand.  This function is very
    /// much intended for use with iterators, rather than manually written.  If you're manully
    /// writing cases, see [`TestContextBuilder::tests`].
    ///
    /// ```
    /// # use erudite::TestContext;
    /// let ctx = TestContext::builder()
    ///     .run_command(["echo", "hi"])
    ///     .test_groups([
    ///         ("group1", [("hello world", "dlrow olleh", true), ("good morning", "gninrom doog", true)]),
    ///         ("group2", [("52", "even", true), ("27", "odd", true)])
    ///     ])
    ///     .build();
    /// ```
    // NOTE: this function in the type-state builder is not perfect, as the caller could pass an
    // iterator with 0 elements, but it's a relatively rare case.
    pub fn test_groups(
        mut self,
        test_groups: impl IntoIterator<Item = (G, impl IntoIterator<Item = impl Into<TestCase<T>>>)>,
    ) -> TestContextBuilder<G, T, SetTests, RunCmd> {
        test_groups.into_iter().for_each(|(group, tests)| {
            self.test_cases
                .entry(group)
                .or_default()
                .extend(tests.into_iter().map(Into::into));
        });
        self.transform()
    }
}

impl<G, T> TestContextBuilder<G, T, SetTests, SetRunCmd>
where
    G: Hash + Eq,
{
    /// Finish building this test context
    pub fn build(self) -> TestContext<G, T> {
        // Just some sanity checks
        assert!(self.command.run().is_some());
        assert!(!self.test_cases.is_empty());
        TestContext {
            trim_output: self.trim_output,
            files: self.files,
            test_cases: self
                .test_cases
                .into_iter()
                .map(|(k, v)| (k, v.into_boxed_slice().into()))
                .collect(),
            command: self.command.into(),
            timeout: self.timeout,
            rules: self.rules,
            max_memory: self.max_memory,
            max_file_size: self.max_file_size,
            max_threads: self.max_threads,
        }
    }
}

#[cfg(test)]
mod test {
    use std::{path::Path, time::Duration};

    use leucite::{MemorySize, Rules};

    use crate::{cases::ExpectedOutput, TestContext};

    #[test]
    fn minimal_builder() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test("group1", "hello", "world", ())
            .build();

        assert_eq!(
            ctx.command.run().map(|x| &**x),
            Some(&["echo".to_string(), "foo".to_string()][..])
        );

        assert_eq!(ctx.test_cases["group1"][0].input(), "hello");
        assert!(
            matches!(ctx.test_cases["group1"][0].output(), ExpectedOutput::String(s) if s == "world")
        );
    }

    #[test]
    fn absolute_file_destination() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "hello", "world", ())
            .file(Path::new("./foo/bar.txt"), "/foo/bar.txt")
            .build();

        assert_eq!(ctx.files[0].dest(), Path::new("foo/bar.txt"));
    }

    #[test]
    fn absolute_files_destination() {
        let ctx = TestContext::builder()
            .run_command(["echo", "foo"])
            .test(0, "hello", "world", ())
            .files([
                (Path::new("./foo/bar.txt"), "/bar.txt"),
                (Path::new("./foo/bar.txt"), "/foo/bar.rs"),
            ])
            .build();

        assert_eq!(ctx.files[0].dest(), Path::new("bar.txt"));
        assert_eq!(ctx.files[1].dest(), Path::new("foo/bar.rs"));
    }

    macro_rules! test_field {
        ($field: ident, $run_field: ident = $a: expr, $compile_field: ident = $b: expr) => {
            concat_idents::concat_idents!(name = $field, _field {
                #[test]
                fn name() {
                    let a = $a;
                    let ctx = TestContext::builder()
                        .run_command(["echo", "foo"])
                        .test(0, "hello", "world", ())
                        .$field(a.clone())
                        .build();

                    assert_eq!(ctx.$field.run(), Some(&a));
                    assert_eq!(ctx.$field.compile(), Some(&a));

                    let b = $b;
                    let ctx = TestContext::builder()
                        .run_command(["echo", "foo"])
                        .test(0, "hello", "world", ())
                        .$run_field(a.clone())
                        .$compile_field(b.clone())
                        .build();

                    assert_eq!(ctx.$field.run(), Some(&a));
                    assert_eq!(ctx.$field.compile(), Some(&b));
                }
            });
        };
    }

    test_field!(
        rules,
        run_rules = Rules::new(),
        compile_rules = Rules::new().add_read_only("/foo")
    );
    test_field!(
        max_memory,
        run_max_memory = MemorySize::from_mib(69),
        compile_max_memory = MemorySize::from_mib(420)
    );
    test_field!(
        max_file_size,
        run_max_file_size = MemorySize::from_mib(69),
        compile_max_file_size = MemorySize::from_mib(420)
    );
    test_field!(max_threads, run_max_threads = 69, compile_max_threads = 420);
    test_field!(
        timeout,
        run_timeout = Duration::from_millis(69),
        compile_timeout = Duration::from_millis(420)
    );

    #[test]
    fn builder_test_tests_equivalent() {
        let singular = TestContext::builder()
            .run_command([""])
            .test(0, "foo", "bar", 5)
            .test(0, "bar", "baz", 6)
            .build();
        let plural = TestContext::builder()
            .run_command([""])
            .tests(0, [("foo", "bar", 5), ("bar", "baz", 6)])
            .build();

        assert_eq!(singular.test_cases, plural.test_cases);
    }

    #[test]
    fn builder_file_files_equivalent() {
        let singular = TestContext::builder()
            .run_command([""])
            .test(0, "", "", 0)
            .file("foo".as_bytes().to_vec(), "bar.rs")
            .file("bar".as_bytes().to_vec(), "baz.rs")
            .file(Path::new("bar"), "baz.txt")
            .file(Path::new("qux"), "qux.txt")
            .build();
        let plural = TestContext::builder()
            .run_command([""])
            .test(0, "", "", 0)
            .files([
                ("foo".as_bytes().to_vec(), "bar.rs"),
                ("bar".as_bytes().to_vec(), "baz.rs"),
            ])
            .files([(Path::new("bar"), "baz.txt"), (Path::new("qux"), "qux.txt")])
            .build();

        assert_eq!(singular.test_cases, plural.test_cases);
    }

    /// A few `compile_fail` tests to confirm the efficacy of the type-state builder
    ///
    /// No run method or tests
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .build();
    /// ```
    ///
    /// One test, but no run method
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .test((), "foo", "bar", ())
    ///     .build();
    /// ```
    ///
    /// Many tests, but no run method
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .test((), "foo", "bar", ())
    ///     .test((), "foo", "bar", ())
    ///     .test((), "foo", "bar", ())
    ///     .build();
    /// ```
    ///
    /// One test using `.tests`, but no run method
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .tests((), [("foo", "bar", ())])
    ///     .build();
    /// ```
    ///
    /// Many tests using `.tests`, but no run method
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .tests((), [("foo", "bar", ()), ("baz", "qux", ())])
    ///     .build();
    /// ```
    ///
    /// Run method, but no tests
    /// ```compile_fail
    /// use erudite::TestContext;
    /// let context = TestContext::builder()
    ///     .run_command(["rustc", "-o", "solution", "solution.rs"])
    ///     .build();
    /// ```
    #[allow(unused)]
    #[doc(hidden)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn type_state_builder_test() {}
}
