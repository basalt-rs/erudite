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

impl ExpectedOutput {
    pub(crate) fn is_valid(&self, output: &str) -> bool {
        match self {
            Self::String(ref s) => s == output,
            #[cfg(feature = "regex")]
            Self::Regex(ref reg) => reg.is_match(output),
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

impl<I, O> From<(I, O)> for TestCase<()>
where
    I: Into<String>,
    O: Into<ExpectedOutput>,
{
    fn from((input, output): (I, O)) -> Self {
        Self::new(input, output, ())
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

#[cfg(test)]
mod test {
    use regex::Regex;

    use crate::cases::{ExpectedOutput, TestCase};

    #[test]
    fn expect_output_is_valid_string() {
        let validator = ExpectedOutput::from("hi");
        assert!(validator.is_valid("hi"));
        assert!(!validator.is_valid("bye"));
    }

    #[test]
    fn expect_output_is_valid_empty_string() {
        let validator = ExpectedOutput::String(String::new());
        assert!(validator.is_valid(""));
        assert!(!validator.is_valid("this is not empty"));
    }

    #[test]
    fn expect_output_is_valid_regex() {
        let validator = ExpectedOutput::from(Regex::new(r"([01]\d|2[0-3])(:[0-5]\d){2}").unwrap());
        assert!(validator.is_valid("04:22:57"));
        assert!(validator.is_valid("14:22:57"));
        assert!(!validator.is_valid("24:22:57"));
    }

    #[test]
    fn test_case_getters() {
        let case = TestCase::new("hello", "world", 69);
        assert_eq!(case.input(), "hello");
        assert!(matches!(case.output(), ExpectedOutput::String(x) if x == "world"));
        assert_eq!(*case.data(), 69);
        assert_eq!(case.into_data(), 69);
        let case2 = TestCase::new("foo", "bar", 420);
        assert_eq!(case2.input(), "foo");
        assert!(matches!(case2.output(), ExpectedOutput::String(x) if x == "bar"));
        assert_eq!(*case2.data(), 420);
        assert_eq!(case2.into_data(), 420);
    }

    #[test]
    fn test_from_tuple2() {
        let case: TestCase<_> = ("hello", "world").into();
        assert_eq!(case.input(), "hello");
        assert!(matches!(case.output(), ExpectedOutput::String(x) if x == "world"));
        let () = *case.data();
        let () = case.into_data();
    }

    #[test]
    fn test_from_tuple3() {
        let case: TestCase<_> = ("hello", "world", 69).into();
        assert_eq!(case.input(), "hello");
        assert!(matches!(case.output(), ExpectedOutput::String(x) if x == "world"));
        assert_eq!(*case.data(), 69);
        assert_eq!(case.into_data(), 69);
        let case2: TestCase<_> = ("foo", "bar", 420).into();
        assert_eq!(case2.input(), "foo");
        assert!(matches!(case2.output(), ExpectedOutput::String(x) if x == "bar"));
        assert_eq!(*case2.data(), 420);
        assert_eq!(case2.into_data(), 420);
    }
}
