//! The CLI's single path to stdout.
//!
//! `println!` panics (exit code 101) when stdout is a pipe whose reader has
//! gone away, as in `graphcal eval model.gcl | head -1`. Every command writes
//! through [`outln!`]/[`out!`] instead, which write to a locked stdout and end
//! the process quietly on a broken pipe; clap's help and version output goes
//! through [`exit_with_clap_output`] for the same reason. The crate does not
//! exempt itself from `clippy::print_stdout`, so a new `println!` fails the
//! lint instead of silently reintroducing the panic.

use std::fmt;
use std::io::{self, Write as _};
use std::process;

/// Exit status after the stdout reader closed the pipe: the conventional
/// shell status of a writer killed by `SIGPIPE` (128 + 13). It is nonzero so
/// that `set -o pipefail` does not mistake truncated output (for example of
/// `format --check`) for success; nothing is printed, as for other tools.
pub const BROKEN_PIPE_EXIT_CODE: i32 = 141;

/// Write formatted text to stdout, followed by a newline when `newline`.
///
/// A broken pipe exits quietly with [`BROKEN_PIPE_EXIT_CODE`]; any other
/// write failure is reported on stderr and exits with status 2.
pub fn write(arguments: fmt::Arguments<'_>, newline: bool) {
    let mut stdout = io::stdout().lock();
    let written = if newline {
        writeln!(stdout, "{arguments}")
    } else {
        write!(stdout, "{arguments}")
    };
    if let Err(error) = written.and_then(|()| stdout.flush()) {
        exit_after_write_error(&error);
    }
}

/// End the process after a failed stdout write.
pub fn exit_after_write_error(error: &io::Error) -> ! {
    if error.kind() == io::ErrorKind::BrokenPipe {
        process::exit(BROKEN_PIPE_EXIT_CODE);
    }
    #[expect(
        clippy::print_stderr,
        reason = "stdout failed; stderr is the only remaining channel"
    )]
    {
        eprintln!("error: could not write to stdout: {error}");
    }
    process::exit(2);
}

/// Print clap's own output (help, version, or a usage error) and exit with
/// clap's status, applying the same closed-pipe policy as [`write`] to
/// stdout output. `clap::Error::exit` ignores every write error and would
/// report `--help | head -1`-style truncation as success.
pub fn exit_with_clap_output(error: &clap::Error) -> ! {
    if let Err(write_error) = error.print()
        && !error.use_stderr()
    {
        exit_after_write_error(&write_error);
    }
    process::exit(error.exit_code());
}

/// `println!` that exits quietly when stdout is a closed pipe.
macro_rules! outln {
    () => {
        $crate::stdout::write(format_args!(""), true)
    };
    ($($argument:tt)*) => {
        $crate::stdout::write(format_args!($($argument)*), true)
    };
}

/// `print!` that exits quietly when stdout is a closed pipe.
macro_rules! out {
    ($($argument:tt)*) => {
        $crate::stdout::write(format_args!($($argument)*), false)
    };
}
