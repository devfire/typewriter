use super::{Cli, parse};
use std::ffi::OsString;
use std::path::PathBuf;

fn args(argv: &[&str]) -> Vec<OsString> {
    argv.iter().map(OsString::from).collect()
}

#[test]
fn no_args_runs_without_a_pdf() {
    assert_eq!(parse(args(&[])), Ok(Cli::Run(None)));
}

#[test]
fn one_path_runs_with_that_pdf() {
    assert_eq!(
        parse(args(&["report.pdf"])),
        Ok(Cli::Run(Some(PathBuf::from("report.pdf"))))
    );
}

#[test]
fn help_and_version_short_circuit() {
    assert_eq!(parse(args(&["--help"])), Ok(Cli::Help));
    assert_eq!(parse(args(&["-h"])), Ok(Cli::Help));
    assert_eq!(parse(args(&["--version"])), Ok(Cli::Version));
    assert_eq!(parse(args(&["-V"])), Ok(Cli::Version));
}

#[test]
fn unknown_option_is_an_error_not_a_filename() {
    let err = parse(args(&["--bogus"])).unwrap_err();
    assert!(err.contains("--bogus"), "unexpected error: {err}");
}

#[test]
fn second_positional_is_an_error() {
    let err = parse(args(&["a.pdf", "b.pdf"])).unwrap_err();
    assert!(err.contains("b.pdf"), "unexpected error: {err}");
}

#[test]
fn double_dash_allows_dash_prefixed_path() {
    assert_eq!(
        parse(args(&["--", "-draft.pdf"])),
        Ok(Cli::Run(Some(PathBuf::from("-draft.pdf"))))
    );
    // After `--`, even `--help` is a filename.
    assert_eq!(
        parse(args(&["--", "--help"])),
        Ok(Cli::Run(Some(PathBuf::from("--help"))))
    );
}

#[test]
fn single_dash_is_a_path() {
    assert_eq!(
        parse(args(&["-"])),
        Ok(Cli::Run(Some(PathBuf::from("-"))))
    );
}
