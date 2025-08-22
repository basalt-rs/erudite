use derive_more::From;

#[derive(Debug, Clone, From)]
pub enum ExpectedOutput {
    String(#[from] String),
    #[cfg(feature = "regex")]
    Regex(#[from] regex::Regex),
}

impl From<&str> for ExpectedOutput {
    fn from(value: &str) -> Self {
        ExpectedOutput::String(value.into())
    }
}

// TODO: output validator trait?
#[derive(Debug, Clone, From)]
pub struct OutputValidator {
    pub(crate) trim_output: bool,
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
            ExpectedOutput::String(ref s) => s == output,
            #[cfg(feature = "regex")]
            ExpectedOutput::Regex(ref reg) => reg.is_match(output),
        }
    }
}

/// A test case which contains input, output, and some associated data
#[derive(Debug, Clone)]
pub struct TestCase<T = ()> {
    pub(crate) input: String,
    pub(crate) output: ExpectedOutput,
    pub(crate) data: T,
}

impl<T> TestCase<T> {
    pub fn new(input: impl Into<String>, output: impl Into<ExpectedOutput>, data: T) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            data,
        }
    }

    /// Retrieve the input value associated with this test case
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Retrieve the expected output for this test case
    pub fn output(&self) -> &ExpectedOutput {
        &self.output
    }

    /// Get the data assocated with this test case
    ///
    /// See also: [`TestCase::into_data`]
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Get the owned data assocated with this test case
    ///
    /// See also: [`TestCase::data`]
    pub fn into_data(self) -> T {
        self.data
    }
}

impl<I, O, T> From<(I, O)> for TestCase<T>
where
    I: Into<String>,
    O: Into<ExpectedOutput>,
    T: Default,
{
    fn from((input, output): (I, O)) -> Self {
        Self::new(input, output, T::default())
    }
}

impl<I, O, T> From<(I, O, T)> for TestCase<T>
where
    I: Into<String>,
    O: Into<ExpectedOutput>,
{
    fn from((input, output, data): (I, O, T)) -> Self {
        Self::new(input, output, data)
    }
}
