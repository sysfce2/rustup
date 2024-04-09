use std::cell::RefCell;
use std::env;
use std::ffi::OsString;
use std::fmt::Debug;
use std::io::{self, IsTerminal};
use std::panic;
use std::path::PathBuf;
use std::sync::Once;
#[cfg(feature = "test")]
use std::{
    collections::HashMap,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
};

#[cfg(feature = "test")]
use rand::{thread_rng, Rng};

pub mod filesource;
pub mod terminalsource;

/// Allows concrete types for the currentprocess abstraction.
#[derive(Clone, Debug)]
pub enum Process {
    Os(OsProcess),
    #[cfg(feature = "test")]
    Test(TestProcess),
}

impl Process {
    /// Obtain the current instance of CurrentProcess
    ///
    /// Panics if no process instance has been set.
    pub fn get() -> Self {
        match PROCESS.with(|p| p.borrow().clone()) {
            Some(p) => p,
            None => panic!("no process instance"),
        }
    }

    pub fn os() -> Self {
        Self::Os(OsProcess {
            stderr_is_a_tty: io::stderr().is_terminal(),
            stdout_is_a_tty: io::stdout().is_terminal(),
        })
    }

    /// Run a function in the context of a process definition.
    ///
    /// If the function panics, the process definition *in that thread* is cleared
    /// by an implicitly installed global panic hook.
    pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
        HOOK_INSTALLED.call_once(|| {
            let orig_hook = panic::take_hook();
            panic::set_hook(Box::new(move |info| {
                clear_process();
                orig_hook(info);
            }));
        });

        PROCESS.with(|p| {
            if let Some(old_p) = &*p.borrow() {
                panic!("current process already set {old_p:?}");
            }
            *p.borrow_mut() = Some(self);
            let result = f();
            *p.borrow_mut() = None;
            result
        })
    }

    pub fn name(&self) -> Option<String> {
        let arg0 = match self.var("RUSTUP_FORCE_ARG0") {
            Ok(v) => Some(v),
            Err(_) => self.args().next(),
        }
        .map(PathBuf::from);

        arg0.as_ref()
            .and_then(|a| a.file_stem())
            .and_then(std::ffi::OsStr::to_str)
            .map(String::from)
    }

    pub fn var(&self, key: &str) -> Result<String, env::VarError> {
        match self {
            Process::Os(_) => env::var(key),
            #[cfg(feature = "test")]
            Process::Test(p) => match p.vars.get(key) {
                Some(val) => Ok(val.to_owned()),
                None => Err(env::VarError::NotPresent),
            },
        }
    }

    pub(crate) fn var_os(&self, key: &str) -> Option<OsString> {
        match self {
            Process::Os(_) => env::var_os(key),
            #[cfg(feature = "test")]
            Process::Test(p) => p.vars.get(key).map(OsString::from),
        }
    }

    pub(crate) fn args(&self) -> Box<dyn Iterator<Item = String> + '_> {
        match self {
            Process::Os(_) => Box::new(env::args()),
            #[cfg(feature = "test")]
            Process::Test(p) => Box::new(p.args.iter().cloned()),
        }
    }

    pub(crate) fn args_os(&self) -> Box<dyn Iterator<Item = OsString> + '_> {
        match self {
            Process::Os(_) => Box::new(env::args_os()),
            #[cfg(feature = "test")]
            Process::Test(p) => Box::new(p.args.iter().map(OsString::from)),
        }
    }

    pub(crate) fn stdin(&self) -> Box<dyn filesource::Stdin> {
        match self {
            Process::Os(_) => Box::new(io::stdin()),
            #[cfg(feature = "test")]
            Process::Test(p) => Box::new(filesource::TestStdin(p.stdin.clone())),
        }
    }

    pub(crate) fn stdout(&self) -> Box<dyn filesource::Writer> {
        match self {
            Process::Os(_) => Box::new(io::stdout()),
            #[cfg(feature = "test")]
            Process::Test(p) => Box::new(filesource::TestWriter(p.stdout.clone())),
        }
    }

    pub(crate) fn stderr(&self) -> Box<dyn filesource::Writer> {
        match self {
            Process::Os(_) => Box::new(io::stderr()),
            #[cfg(feature = "test")]
            Process::Test(p) => Box::new(filesource::TestWriter(p.stderr.clone())),
        }
    }

    pub(crate) fn current_dir(&self) -> io::Result<PathBuf> {
        match self {
            Process::Os(_) => env::current_dir(),
            #[cfg(feature = "test")]
            Process::Test(p) => Ok(p.cwd.clone()),
        }
    }

    #[cfg(test)]
    fn id(&self) -> u64 {
        match self {
            Process::Os(_) => std::process::id() as u64,
            #[cfg(feature = "test")]
            Process::Test(p) => p.id,
        }
    }
}

impl home::env::Env for Process {
    fn home_dir(&self) -> Option<PathBuf> {
        match self {
            Process::Os(_) => self.var("HOME").ok().map(|v| v.into()),
            #[cfg(feature = "test")]
            Process::Test(_) => home::env::OS_ENV.home_dir(),
        }
    }

    fn current_dir(&self) -> Result<PathBuf, io::Error> {
        match self {
            Process::Os(_) => self.current_dir(),
            #[cfg(feature = "test")]
            Process::Test(_) => home::env::OS_ENV.current_dir(),
        }
    }

    fn var_os(&self, key: &str) -> Option<OsString> {
        match self {
            Process::Os(_) => self.var_os(key),
            #[cfg(feature = "test")]
            Process::Test(_) => self.var_os(key),
        }
    }
}

#[cfg(feature = "test")]
impl From<TestProcess> for Process {
    fn from(p: TestProcess) -> Self {
        Self::Test(p)
    }
}

static HOOK_INSTALLED: Once = Once::new();

/// Internal - for the panic hook only
fn clear_process() {
    PROCESS.with(|p| p.replace(None));
}

thread_local! {
    static PROCESS: RefCell<Option<Process>> = const { RefCell::new(None) };
}

// ----------- real process -----------------

#[derive(Clone, Debug)]
pub struct OsProcess {
    pub(self) stderr_is_a_tty: bool,
    pub(self) stdout_is_a_tty: bool,
}

// ------------ test process ----------------
#[cfg(feature = "test")]
#[derive(Clone, Debug, Default)]
pub struct TestProcess {
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub vars: HashMap<String, String>,
    pub id: u64,
    pub stdin: filesource::TestStdinInner,
    pub stdout: filesource::TestWriterInner,
    pub stderr: filesource::TestWriterInner,
}

#[cfg(feature = "test")]
impl TestProcess {
    pub fn new<P: AsRef<Path>, A: AsRef<str>>(
        cwd: P,
        args: &[A],
        vars: HashMap<String, String>,
        stdin: &str,
    ) -> Self {
        TestProcess {
            cwd: cwd.as_ref().to_path_buf(),
            args: args.iter().map(|s| s.as_ref().to_string()).collect(),
            vars,
            id: TestProcess::new_id(),
            stdin: Arc::new(Mutex::new(Cursor::new(stdin.to_string()))),
            stdout: Arc::new(Mutex::new(Vec::new())),
            stderr: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn run<R>(self, f: impl FnOnce() -> R) -> R {
        Process::from(self).run(f)
    }

    fn new_id() -> u64 {
        let low_bits: u64 = std::process::id() as u64;
        let mut rng = thread_rng();
        let high_bits = rng.gen_range(0..u32::MAX) as u64;
        high_bits << 32 | low_bits
    }

    /// Extracts the stdout from the process
    pub fn get_stdout(&self) -> Vec<u8> {
        self.stdout
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Extracts the stderr from the process
    pub fn get_stderr(&self) -> Vec<u8> {
        self.stderr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;

    use rustup_macros::unit_test as test;

    use super::{Process, TestProcess};

    #[test]
    fn test_instance() {
        let proc = TestProcess::new(
            env::current_dir().unwrap(),
            &["foo", "bar", "baz"],
            HashMap::default(),
            "",
        );

        proc.clone().run(|| {
            let cur = Process::get();
            assert_eq!(proc.id, cur.id(), "{:?} != {:?}", proc, cur)
        });
    }
}
