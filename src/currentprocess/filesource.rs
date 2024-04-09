use std::io::{self, BufRead, Cursor, Read, Result, Write};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::currentprocess::process;

use super::terminalsource::{ColorableTerminal, StreamSelector};

/// Stand-in for std::io::Stdin
pub trait Stdin {
    fn lock(&self) -> Box<dyn StdinLock + '_>;
    fn read_line(&self, buf: &mut String) -> Result<usize>;
}

/// Stand-in for std::io::StdinLock
pub trait StdinLock: Read + BufRead {}

// ----------------- OS support for stdin -----------------

impl StdinLock for io::StdinLock<'_> {}

impl Stdin for io::Stdin {
    fn lock(&self) -> Box<dyn StdinLock + '_> {
        Box::new(io::Stdin::lock(self))
    }

    fn read_line(&self, buf: &mut String) -> Result<usize> {
        io::Stdin::read_line(self, buf)
    }
}

// ----------------------- test support for stdin ------------------

struct TestStdinLock<'a> {
    inner: MutexGuard<'a, Cursor<String>>,
}

impl StdinLock for TestStdinLock<'_> {}

impl Read for TestStdinLock<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl BufRead for TestStdinLock<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.inner.consume(n)
    }
}

pub(crate) type TestStdinInner = Arc<Mutex<Cursor<String>>>;

pub(super) struct TestStdin(pub(super) TestStdinInner);

impl Stdin for TestStdin {
    fn lock(&self) -> Box<dyn StdinLock + '_> {
        Box::new(TestStdinLock {
            inner: self.0.lock().unwrap_or_else(|e| e.into_inner()),
        })
    }
    fn read_line(&self, buf: &mut String) -> Result<usize> {
        self.lock().read_line(buf)
    }
}

// -------------- stdout -------------------------------

/// This is a stand-in for [`std::io::StdoutLock`] and [`std::io::StderrLock`].
pub trait WriterLock: Write {}

/// This is a stand-in for [`std::io::Stdout`] or [`std::io::Stderr`].
/// TODO: remove Sync.
pub trait Writer: Write + Send + Sync {
    /// This is a stand-in for [`std::io::Stdout::lock`] or [`std::io::Stderr::lock`].
    fn lock(&self) -> Box<dyn WriterLock + '_>;

    /// Query whether a TTY is present. Used in download_tracker - we may want
    /// to remove this entirely with a better progress bar system (in favour of
    /// filtering in the Terminal layer?)
    fn is_a_tty(&self) -> bool;

    /// Construct a terminal on this writer.
    fn terminal(&self) -> ColorableTerminal;
}

// ----------------- OS support for writers -----------------

impl WriterLock for io::StdoutLock<'_> {}

impl Writer for io::Stdout {
    fn is_a_tty(&self) -> bool {
        match process() {
            crate::currentprocess::Process::Os(p) => p.stdout_is_a_tty,
            #[cfg(feature = "test")]
            crate::currentprocess::Process::Test(_) => unreachable!(),
        }
    }

    fn lock(&self) -> Box<dyn WriterLock + '_> {
        Box::new(io::Stdout::lock(self))
    }

    fn terminal(&self) -> ColorableTerminal {
        ColorableTerminal::new(StreamSelector::Stdout)
    }
}

impl WriterLock for io::StderrLock<'_> {}

impl Writer for io::Stderr {
    fn is_a_tty(&self) -> bool {
        match process() {
            crate::currentprocess::Process::Os(p) => p.stderr_is_a_tty,
            #[cfg(feature = "test")]
            crate::currentprocess::Process::Test(_) => unreachable!(),
        }
    }

    fn lock(&self) -> Box<dyn WriterLock + '_> {
        Box::new(io::Stderr::lock(self))
    }

    fn terminal(&self) -> ColorableTerminal {
        ColorableTerminal::new(StreamSelector::Stderr)
    }
}

// ----------------------- test support for writers ------------------

pub(super) struct TestWriterLock<'a> {
    inner: MutexGuard<'a, Vec<u8>>,
}

impl WriterLock for TestWriterLock<'_> {}

impl Write for TestWriterLock<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(feature = "test")]
pub(super) type TestWriterInner = Arc<Mutex<Vec<u8>>>;
/// A thread-safe test file handle that pretends to be e.g. stdout.
#[derive(Clone, Default)]
#[cfg(feature = "test")]
pub(super) struct TestWriter(pub(super) TestWriterInner);

#[cfg(feature = "test")]
impl TestWriter {
    pub(super) fn lock(&self) -> TestWriterLock<'_> {
        // The stream can be locked even if a test thread panicked: its state
        // will be ok
        TestWriterLock {
            inner: self.0.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

#[cfg(feature = "test")]
impl Writer for TestWriter {
    fn is_a_tty(&self) -> bool {
        false
    }

    fn lock(&self) -> Box<dyn WriterLock + '_> {
        Box::new(self.lock())
    }

    fn terminal(&self) -> ColorableTerminal {
        ColorableTerminal::new(StreamSelector::TestWriter(self.clone()))
    }
}

#[cfg(feature = "test")]
impl Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.lock().write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
