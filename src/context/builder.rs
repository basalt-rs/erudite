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
    UnsetTests => SetTests,
    UnsetRunCmd => SetRunCmd,
}

// NOTE: Ensure that the generics do not affect the layout of this structure.  If a change like
// that is necessary, the `transform` function must change.
pub struct TestContextBuilder<Tests, RunCmd> {
    test_cases: Vec<TestCase>,                // required (at least once)
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

impl TestContextBuilder<UnsetTests, UnsetRunCmd> {
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

impl Default for TestContextBuilder<UnsetTests, UnsetRunCmd> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A, B> TestContextBuilder<A, B> {
    /// Convert a TestContextBuilder<A, B> into TestContextBuilder<C, D>
    // NOTE: This function _must not_ be made public in any way, or the type-state builder can be
    // invalidated.
    fn transform<C, D>(&mut self) -> &mut TestContextBuilder<C, D> {
        // SAFETY: The generics don't affect anything about the actual data here as they only
        // affect the PhantomData, so TestContextBuilder<A, B> has the same layout as
        // TestContextBuilder<C, D>
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
    ($self: ident, $field: ident, $type: ty, $noun: literal) => {
        concat_idents::concat_idents!(ident = run_, $field {
            #[doc = concat!("Set the ", $noun, " when running the program")]
            pub fn ident(&mut $self, $field: $type) -> &mut Self {
                $self.$field.with_run($field);
                $self
            }
        });
        concat_idents::concat_idents!(ident = compile_, $field {
            #[doc = concat!("Set the ", $noun, " when compiling the program")]
            pub fn ident(&mut $self, $field: $type) -> &mut Self {
                $self.$field.with_compile($field);
                $self
            }
        });

        builder_fn!(
            #[doc = concat!("Set the ", $noun, " of _both_ running and compiling the program")]
            fn $field(self, $field: $type) self.$field.with_both($field)
        );
    };
}

// Optional fields
impl<Tests, RunCmd> TestContextBuilder<Tests, RunCmd> {
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
                dst: destination,
                src: source.into(),
            });
        }
    );

    command_config_fns!(self, rules, Rules, "rules for execution");
    command_config_fns!(self, max_memory, MemorySize, "maximum memory usage");
    command_config_fns!(self, max_file_size, MemorySize, "maximum file size");
    command_config_fns!(self, max_threads, u64, "max thread count");
    command_config_fns!(self, timeout, Duration, "timeout");
}

// `.run_command` when no command has been added
impl<Tests> TestContextBuilder<Tests, UnsetRunCmd> {
    pub fn run_command(
        &mut self,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut TestContextBuilder<Tests, SetRunCmd> {
        self.command
            .with_run(command.into_iter().map(Into::into).collect());
        self.transform()
    }
}

// Tests when none have been added
impl<RunCmd> TestContextBuilder<UnsetTests, RunCmd> {
    pub fn test(
        &mut self,
        input: impl Into<String>,
        output: impl Into<ExpectedOutput>,
    ) -> &mut TestContextBuilder<SetTests, RunCmd> {
        self.test_cases.push(TestCase::new(input, output));
        self.transform()
    }

    pub fn tests(
        &mut self,
        tests: impl IntoIterator<Item = impl Into<TestCase>>,
    ) -> &mut TestContextBuilder<SetTests, RunCmd> {
        self.test_cases.extend(tests.into_iter().map(Into::into));
        self.transform()
    }
}

// Tests when >= 1 have been added
impl<RunCmd> TestContextBuilder<SetTests, RunCmd> {
    pub fn test(
        &mut self,
        input: impl Into<String>,
        output: impl Into<ExpectedOutput>,
    ) -> &mut Self {
        self.test_cases.push(TestCase::new(input, output));
        self
    }

    pub fn tests(&mut self, tests: impl IntoIterator<Item = impl Into<TestCase>>) -> &mut Self {
        self.test_cases.extend(tests.into_iter().map(Into::into));
        self
    }
}

impl TestContextBuilder<SetTests, SetRunCmd> {
    pub fn build(&mut self) -> TestContext {
        // `Clone`s are sad, but I feel like `std::mem::take` would be kind of weird behaviour
        // here.
        // We also can't accept `self` because then the chaining won't work.
        TestContext {
            trim_output: self.trim_output,
            files: self.files.clone(),
            command: self.command.clone().into(),
            timeout: self.timeout,
            rules: self.rules.clone(),
            max_memory: self.max_memory,
            max_file_size: self.max_file_size,
            max_threads: self.max_threads,
        }
    }
}
