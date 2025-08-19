use super::{CommandConfig, ExpectedOutput, FileConfig, FileContent, TestCase, TestContext};
use std::{marker::PhantomData, path::PathBuf, time::Duration};

use leucite::{MemorySize, Rules};

macro_rules! define_state_structs {
    ($($unset: ident => $set: ident),+$(,)?) => {
        $(define_state_structs!(@ $unset, $set);)+
    };
    (@ $($name: ident),+$(,)?) => {
        $(
            #[non_exhaustive]
            pub struct $name;
        )+
    };
}

define_state_structs! {
    MissingTests => SetTests,
    MissingRunCmd => SetRunCmd,
}

// NOTE: Ensure that the generics do not affect the layout of this structure.  If a change like
// that is necessary, the `transform` function must change.
pub struct TestContextBuilder<Tests, RunCmd, T: 'static = ()> {
    test_cases: Vec<TestCase<T>>,             // required (at least once)
    trim_output: bool,                        // optional
    files: Vec<FileConfig>,                   // optional
    command: CommandConfig<Vec<String>>,      // Compile: optional, Run: required
    timeout: CommandConfig<Duration>,         // optional
    rules: CommandConfig<Rules>,              // optional
    max_memory: CommandConfig<MemorySize>,    // optional
    max_file_size: CommandConfig<MemorySize>, // optional
    max_threads: CommandConfig<u64>,          // optional

    state: PhantomData<(Tests, RunCmd)>,
}

impl<T> TestContextBuilder<MissingTests, MissingRunCmd, T> {
    pub fn new() -> Self {
        Self {
            trim_output: true,
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

impl<T> Default for TestContextBuilder<MissingTests, MissingRunCmd, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A, B, T> TestContextBuilder<A, B, T> {
    /// Convert a TestContextBuilder<A, B, T> into TestContextBuilder<C, D, T>
    // NOTE: This function _must not_ be made public in any way, or the type-state builder can be
    // invalidated.
    fn transform<C, D>(&mut self) -> &mut TestContextBuilder<C, D, T> {
        // SAFETY: The A/B/C/D generics don't affect anything about the actual data here and the
        // `T` generic stays the same, so `TestContextBuilder<A, B, T>` has the same layout as
        // `TestContextBuilder<C, D, T>`
        unsafe { std::mem::transmute(self) }
    }
}

macro_rules! builder_fn {
    ($(#[$($doc: tt)+])* fn $name: ident($self: ident, $($field: ident: $type: ty),+$(,)?) $body: expr) => {
        $(#[$($doc)+])*
        pub fn $name(&mut $self, $($field: $type),+) -> &mut Self {
            $body;
            $self
        }
    };
    ($($(#[$($doc: tt)+])* fn $name: ident($self: ident, $($field: ident: $type: ty),+$(,)?) $body: expr)+) => {
        $(builder_fn!(
            $(#[$($doc)+])*
            fn $name($self, $($field: $type),+) $body
        );)+
    };
}

macro_rules! command_config_fns {
    ($field: ident, $type: ty, $noun: literal) => {
        concat_idents::concat_idents!(ident = run_, $field {
            #[doc = concat!("Set the ", $noun, " when running the program")]
            pub fn ident(&mut self, $field: $type) -> &mut Self {
                self.$field.with_run($field);
                self
            }
        });
        concat_idents::concat_idents!(ident = compile_, $field {
            #[doc = concat!("Set the ", $noun, " when compiling the program")]
            pub fn ident(&mut self, $field: $type) -> &mut Self {
                self.$field.with_compile($field);
                self
            }
        });

        builder_fn!(
            #[doc = concat!("Set the ", $noun, " of _both_ running and compiling the program")]
            fn $field(self, $field: $type) self.$field.with_both($field)
        );
    };
}

// Optional fields
impl<Tests, RunCmd, T> TestContextBuilder<Tests, RunCmd, T> {
    builder_fn!(
        fn trim_output(self, trim_output: bool) self.trim_output = trim_output

        fn compile_command(self, compile_command: impl IntoIterator<Item = impl Into<String>>)
            self.command
                .with_compile(compile_command.into_iter().map(Into::into).collect())

        /// Add a file to be inserted into the runtime environment of the test.  This gets added
        /// before compilation, so it works well for libraries or input/output manipulation.
        ///
        /// # Note
        ///
        /// `destination` is relative to the directory used for the compilation/runtime environment
        ///
        /// # Panics
        ///
        /// - If `destination` is not a relative path
        fn file(self, source: impl Into<FileContent>, destination: impl Into<PathBuf>) {
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
        }
    );

    command_config_fns!(rules, Rules, "rules for execution");
    command_config_fns!(max_memory, MemorySize, "maximum memory usage");
    command_config_fns!(max_file_size, MemorySize, "maximum file size");
    command_config_fns!(max_threads, u64, "max thread count");
    command_config_fns!(timeout, Duration, "timeout");
}

// `.run_command` when no command has been added
impl<Tests, T> TestContextBuilder<Tests, MissingRunCmd, T> {
    pub fn run_command(
        &mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut TestContextBuilder<Tests, SetRunCmd, T> {
        self.command
            .with_run(command.into_iter().map(Into::into).collect());
        self.transform()
    }
}

impl<Tests, RunCmd, T> TestContextBuilder<Tests, RunCmd, T> {
    pub fn test(
        &mut self,
        input: impl Into<String>,
        output: impl Into<ExpectedOutput>,
        data: T,
    ) -> &mut TestContextBuilder<SetTests, RunCmd, T> {
        self.test_cases.push(TestCase::new(input, output, data));
        self.transform()
    }

    pub fn tests(
        &mut self,
        tests: impl IntoIterator<Item = impl Into<TestCase<T>>>,
    ) -> &mut TestContextBuilder<SetTests, RunCmd, T> {
        self.test_cases.extend(tests.into_iter().map(Into::into));
        self.transform()
    }
}

impl<T> TestContextBuilder<SetTests, SetRunCmd, T>
where
    T: Clone,
{
    /// Build the [`TestContext`] by cloning the data in the builder.  This is the recommended
    /// approach, but it may not work for everything as it requires `T` to be [`Clone`].  If `T` is not
    /// [`Clone`], consider using [`Self::build_and_reset`] or [`Self::build_and_consume`]
    pub fn build(&self) -> TestContext<T> {
        TestContext {
            trim_output: self.trim_output,
            files: self.files.clone(),
            test_cases: self.test_cases.clone(),
            command: self.command.clone().into(),
            timeout: self.timeout,
            rules: self.rules.clone(),
            max_memory: self.max_memory,
            max_file_size: self.max_file_size,
            max_threads: self.max_threads,
        }
    }
}

impl<T> TestContextBuilder<SetTests, SetRunCmd, T> {
    /// Build the [`TestContext`] and reset the builder the builder to the default state.  This is
    /// useful if the `T` is not [`Clone`].  If `T` is clone, consider using [`Self::build`].
    pub fn build_and_reset(&mut self) -> TestContext<T> {
        let this =
            std::mem::take::<TestContextBuilder<MissingTests, MissingRunCmd, T>>(self.transform());
        TestContext {
            trim_output: this.trim_output,
            files: this.files,
            test_cases: this.test_cases,
            command: this.command.into(),
            timeout: this.timeout,
            rules: this.rules,
            max_memory: this.max_memory,
            max_file_size: this.max_file_size,
            max_threads: this.max_threads,
        }
    }
}

impl<T> TestContextBuilder<SetTests, SetRunCmd, T> {
    /// Build the [`TestContext`] by consuming the builder.
    ///
    /// See also: [`Self::build`] and [`Self::build_and_reset`]
    pub fn build_and_consume(self) -> TestContext<T> {
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
