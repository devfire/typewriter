//! Command-line parsing: `typewriter [OPTIONS] [PDF]`.
//!
//! Hand-rolled (no parser dependency): the surface is one optional
//! positional path plus `--help`/`--version`. Anything else is an
//! error, so a typo'd flag fails loudly instead of being treated as
//! a filename.

use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(test)]
mod tests;

/// Usage printed by `--help` and after argument errors.
pub const USAGE: &str = concat!(
    "Typewriter PDF — annotate a PDF with typed text and signatures\n\
\n\
Usage: ",
    env!("CARGO_PKG_NAME"),
    " [OPTIONS] [PDF]\n\
\n\
Arguments:\n\
  [PDF]  PDF file to open on startup\n\
\n\
Options:\n\
  -h, --help     Print this help and exit\n\
  -V, --version  Print the version and exit\n"
);

/// What `main` should do after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// Launch the GUI, opening `Some(path)` if one was given.
    Run(Option<PathBuf>),
    /// Print [`USAGE`] to stdout and exit successfully.
    Help,
    /// Print the version to stdout and exit successfully.
    Version,
}

/// Parse arguments excluding the program name.
///
/// `--` ends option parsing, so a path starting with `-` can still
/// be opened (`typewriter -- -draft.pdf`). Non-UTF-8 paths are kept
/// as-is via [`OsString`]; only ASCII flags are interpreted.
///
/// # Errors
/// Returns a human-readable message for an unknown option or a
/// second positional argument.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    let mut pdf: Option<PathBuf> = None;
    let mut only_positional = false;
    for arg in args {
        if !only_positional {
            match arg.to_str() {
                Some("--") => {
                    only_positional = true;
                    continue;
                }
                Some("-h" | "--help") => return Ok(Cli::Help),
                Some("-V" | "--version") => return Ok(Cli::Version),
                Some(flag) if flag.len() > 1 && flag.starts_with('-') => {
                    return Err(format!("unknown option: {flag}"));
                }
                _ => {}
            }
        }
        let path = PathBuf::from(arg);
        if let Some(first) = pdf {
            return Err(format!(
                "unexpected extra argument: {} (already have {})",
                path.display(),
                first.display()
            ));
        }
        pdf = Some(path);
    }
    Ok(Cli::Run(pdf))
}
