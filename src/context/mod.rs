use std::{collections::HashMap, hash::Hash, sync::Arc, time::Duration};

use leucite::{MemorySize, Rules};

mod builder;
pub use builder::TestContextBuilder;

use crate::{cases::TestCase, runner::TestRunner, FileConfig};

// TODO: rename (and update test names)
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum StageConfig<T> {
    None,
    Compile(T),
    Run(T),
    Equal(T),
    Different { compile: T, run: T },
}

impl<T> Default for StageConfig<T> {
    fn default() -> Self {
        Self::None
    }
}

impl<T> StageConfig<T> {
    // Can't use From/Into traits because T might be the same as U
    pub fn into<U>(self) -> StageConfig<U>
    where
        U: From<T>,
    {
        match self {
            StageConfig::None => StageConfig::None,
            StageConfig::Compile(c) => StageConfig::Compile(c.into()),
            StageConfig::Run(r) => StageConfig::Run(r.into()),
            StageConfig::Equal(t) => StageConfig::Equal(t.into()),
            StageConfig::Different { compile, run } => StageConfig::Different {
                compile: compile.into(),
                run: run.into(),
            },
        }
    }

    pub fn with_compile(&mut self, compile: T) -> &mut Self {
        let old = std::mem::take(self);
        *self = match old {
            Self::None => Self::Compile(compile),
            Self::Compile(_) => Self::Compile(compile),
            Self::Run(run) => Self::Different { compile, run },
            Self::Equal(run) => Self::Different { compile, run },
            Self::Different { compile: _, run } => Self::Different { compile, run },
        };
        self
    }

    pub fn with_run(&mut self, run: T) -> &mut Self {
        let old = std::mem::take(self);
        *self = match old {
            Self::None => Self::Run(run),
            Self::Compile(compile) => Self::Different { compile, run },
            Self::Run(_) => Self::Run(run),
            Self::Equal(compile) => Self::Different { compile, run },
            Self::Different { compile, run: _ } => Self::Different { compile, run },
        };
        self
    }

    pub fn with_both(&mut self, both: T) -> &mut Self {
        *self = Self::Equal(both);
        self
    }

    pub fn compile(&self) -> Option<&T> {
        match self {
            StageConfig::None => None,
            StageConfig::Compile(c) => Some(c),
            StageConfig::Run(_) => None,
            StageConfig::Equal(c) => Some(c),
            StageConfig::Different { compile, run: _ } => Some(compile),
        }
    }

    pub fn run(&self) -> Option<&T> {
        match self {
            StageConfig::None => None,
            StageConfig::Compile(_) => None,
            StageConfig::Run(r) => Some(r),
            StageConfig::Equal(r) => Some(r),
            StageConfig::Different { compile: _, run } => Some(run),
        }
    }
}

/// Context around a test suite.  This contains information about how a test suite is supposed to
/// be run.
///
/// The idea is that this can be placed into an [`Arc`] within the application state and runners
/// can be created from any thread using [`TestContext::test_runner`].
///
/// # Usage
///
/// Construct one using [`TestContext::builder`]:
///
/// ```
/// # use erudite::{TestContext, FileContent, Rules, MemorySize};
/// # use std::time::Duration;
/// # let rules = Rules::new();
/// let context = TestContext::builder()
///     .compile_command(["rustc", "-o", "main", "main.rs"])
///     .run_command(["./main"])
///     .test("group", "hello", "olleh", ())
///     .test("group", "world", "dlrow", ())
///     .test("group", "rust", "tsur", ())
///     .test("group", "tacocat", "tacocat", ())
///     .trim_output(true)
///     .file(FileContent::string("// some rust code"), "main.rs")
///     .timeout(Duration::from_secs(5))
///     .rules(rules)
///     .max_memory(MemorySize::from_gib(1))
///     .max_file_size(MemorySize::from_gib(1))
///     .max_threads(2)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct TestContext<G, T> {
    pub(crate) trim_output: bool,
    pub(crate) files: Vec<FileConfig>,
    pub(crate) test_cases: HashMap<G, Arc<[TestCase<T>]>>,
    pub(crate) command: StageConfig<Box<[String]>>,
    pub(crate) timeout: StageConfig<Duration>,
    pub(crate) rules: StageConfig<Rules>,
    pub(crate) max_memory: StageConfig<MemorySize>,
    pub(crate) max_file_size: StageConfig<MemorySize>,
    pub(crate) max_threads: StageConfig<u64>,
}

impl<G, T> TestContext<G, T>
where
    G: Hash + Eq,
{
    /// Construct a builder for [`TestContext`], see [`TestContextBuilder`] for more details.
    pub fn builder() -> TestContextBuilder<G, T> {
        TestContextBuilder::new()
    }

    /// Create a [`TestRunner`] from this context.  See [`TestRunner`] for more details.
    pub fn test_runner<'a, Q>(self: Arc<Self>, group: &Q) -> Option<TestRunner<'a, G, T>>
    where
        G: std::borrow::Borrow<Q>,
        Q: Hash + Eq,
    {
        let cases = Arc::clone(self.test_cases.get(group)?);
        Some(TestRunner::new(self, cases))
    }
}

impl<T> TestContext<(), T> {
    /// Create a [`TestRunner`] from this context if the group is `()`.  This does not need to
    /// return `Option` like [`TestContext::test_runner`], as it is guanteed by the builder that
    /// there is an entry.
    pub fn default_test_runner<'a>(self: Arc<Self>) -> TestRunner<'a, (), T> {
        assert!(self.test_cases.len() == 1);
        let cases = Arc::clone(self.test_cases.get(&()).expect("asserted above"));
        TestRunner::new(self, cases)
    }
}

#[cfg(test)]
mod test {
    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicI32, Ordering},
    };

    use tmpdir::TmpDir;

    use crate::{
        context::{FileConfig, StageConfig},
        FileContent,
    };

    #[tokio::test]
    async fn file_config_path() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();
        let input = tmpdir.as_ref().join("in.rs");
        tokio::fs::write(&input, "some content")
            .await
            .expect("failed setting up test");

        let config = FileConfig::new(input, "out.rs");
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content");
    }

    #[tokio::test]
    async fn file_config_path_dir() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();
        let source = tmpdir.as_ref().join("foo");
        tokio::fs::create_dir_all(&source).await.unwrap();
        tokio::fs::write(source.join("bar.txt"), "Some content in /foo/bar.txt")
            .await
            .unwrap();

        let config = FileConfig::new(source, "out");
        assert_eq!(config.dest(), Path::new("out"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let dir = tokio::fs::metadata(tmpdir.as_ref().join("foo"))
            .await
            .expect("failed while reading file");
        assert!(dir.is_dir());
        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("foo/bar.txt"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "Some content in /foo/bar.txt");
    }

    #[tokio::test]
    async fn file_config_bytes() {
        let tmpdir = TmpDir::new("erudite-test").await.unwrap();

        let config = FileConfig::new(String::from("some content").into_bytes(), "out.rs");
        assert_eq!(config.dest(), Path::new("out.rs"));
        config
            .write_file(&tmpdir)
            .await
            .expect("failed while copying file");

        let read = tokio::fs::read_to_string(tmpdir.as_ref().join("out.rs"))
            .await
            .expect("failed while reading file");
        assert_eq!(read, "some content");
    }

    #[test]
    fn file_config_from_tuple2() {
        let cfg: FileConfig = (Path::new("foo/bar"), "foo/bar").into();
        assert_eq!(cfg, FileConfig::new(Path::new("foo/bar"), "foo/bar"));
    }

    #[test]
    fn file_content_path() {
        let content = FileContent::path("foo/bar");
        assert_eq!(content, FileContent::Path(PathBuf::from("foo/bar")));
        let content: FileContent = Path::new("foo/bar").into();
        assert_eq!(content, FileContent::Path(PathBuf::from("foo/bar")));
    }

    #[test]
    fn file_content_string() {
        let content = FileContent::string("hello world");
        assert_eq!(
            content,
            FileContent::Bytes("hello world".as_bytes().to_vec())
        );
    }

    #[test]
    fn file_content_bytes() {
        let bytes = vec![0xca, 0xfe, 0xba, 0xbe];
        let content = FileContent::bytes(bytes.clone());
        assert_eq!(content, FileContent::Bytes(bytes));
    }

    #[test]
    fn stage_config_into() {
        let mut cfg = StageConfig::<&str>::default();

        assert_eq!(cfg.into(), StageConfig::<String>::None);

        cfg.with_run("run");
        assert_eq!(cfg.into(), StageConfig::<String>::Run("run".to_string()));

        cfg.with_compile("compile");
        assert_eq!(
            cfg.into(),
            StageConfig::<String>::Different {
                run: "run".to_string(),
                compile: "compile".to_string()
            }
        );

        let mut cfg = StageConfig::<&str>::default();

        cfg.with_compile("compile");
        assert_eq!(
            cfg.into(),
            StageConfig::<String>::Compile("compile".to_string())
        );

        cfg.with_both("both");
        assert_eq!(cfg.into(), StageConfig::<String>::Equal("both".to_string()));
    }

    #[test]
    fn stage_config_run_only() {
        let mut cfg = StageConfig::default();

        cfg.with_run(42);
        assert_eq!(cfg.run(), Some(&42));
        assert_eq!(cfg.compile(), None);

        cfg.with_run(9001);
        assert_eq!(cfg.run(), Some(&9001));
        assert_eq!(cfg.compile(), None);

        cfg.with_both(1);
        assert_eq!(cfg.run(), Some(&1));
        assert_eq!(cfg.compile(), Some(&1));

        cfg.with_run(42);
        cfg.with_compile(2);
        assert_eq!(cfg.run(), Some(&42));
        assert_eq!(cfg.compile(), Some(&2));
    }

    #[test]
    fn stage_config_compile_only() {
        let mut cfg = StageConfig::default();

        cfg.with_compile(42);
        assert_eq!(cfg.run(), None);
        assert_eq!(cfg.compile(), Some(&42));

        cfg.with_compile(9001);
        assert_eq!(cfg.run(), None);
        assert_eq!(cfg.compile(), Some(&9001));

        cfg.with_both(1);
        assert_eq!(cfg.run(), Some(&1));
        assert_eq!(cfg.compile(), Some(&1));

        cfg.with_compile(42);
        cfg.with_run(2);
        assert_eq!(cfg.run(), Some(&2));
        assert_eq!(cfg.compile(), Some(&42));
    }

    #[test]
    fn stage_config_equal() {
        let mut cfg = StageConfig::default();

        cfg.with_both(AtomicI32::new(0));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(0));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(0));

        // Change the atomic to ensure that they are both pointing at the same value
        cfg.run().unwrap().store(42, Ordering::SeqCst);
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(42));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(42));

        cfg.with_run(AtomicI32::new(1));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(1));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(42));

        cfg.with_both(AtomicI32::new(69));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(69));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(69));

        cfg.with_compile(AtomicI32::new(8));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(69));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(8));
    }

    #[test]
    fn stage_config_different() {
        let mut cfg = StageConfig::default();
        let rval = AtomicI32::new(4);
        let cval = AtomicI32::new(2);
        cfg.with_run(rval).with_compile(cval);

        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(4));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(2));

        // Change the atomic to ensure that they are both pointing at the same value
        cfg.run().unwrap().store(42, Ordering::SeqCst);
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(42));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(2));

        cfg.with_both(AtomicI32::new(1337));
        assert_eq!(cfg.run().map(|x| x.load(Ordering::SeqCst)), Some(1337));
        assert_eq!(cfg.compile().map(|x| x.load(Ordering::SeqCst)), Some(1337));
    }
}
