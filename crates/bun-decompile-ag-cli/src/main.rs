//! CLI for `bun-decompile-ag`. Port of `src/cli.ts`.
//!
//! Argument parsing is hand-rolled rather than clap so that help text,
//! error messages, and exit codes stay byte-identical to the TS CLI — the
//! parity harness diffs them. The surface is four flags; revisit if it
//! grows.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::exit;

use bun_decompile_ag::{ExtractOptions, extract_bundled_files, get_executable_version};

mod debug;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        "bun-decompile v{VERSION}

Extracts bundled source files from a Bun-compiled binary.

Usage:
  bun-decompile <input-binary> [options]
  bun-decompile debug <input-binary>

Options:
  -o, --output <dir>   Output directory (default: ./decompiled)
  --no-normalize       Don't normalize entrypoint filename to index.js
  -v, --version        Print version and exit
  -h, --help           Print this help and exit

Commands:
  debug <binary>       Print the binary's container, format, and offsets"
    );
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Extract {
        input_path: PathBuf,
        output_dir: PathBuf,
        no_normalize: bool,
    },
    /// `debug <binary>`: dump the binary's structure.
    Debug {
        input_path: PathBuf,
    },
    /// Print help and exit with `code` (0 for -h, 1 for a missing input).
    Help {
        code: i32,
    },
    Version,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    // `debug` is a subcommand, valid only as the first argument.
    if args.first().map(String::as_str) == Some("debug") {
        return match args.get(1) {
            Some(path) => Ok(Command::Debug {
                input_path: PathBuf::from(path),
            }),
            None => Err("debug requires a binary path".to_owned()),
        };
    }

    let mut input_path: Option<PathBuf> = None;
    let mut output_dir = PathBuf::from("./decompiled");
    let mut no_normalize = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help { code: 0 }),
            "-v" | "--version" => return Ok(Command::Version),
            "--no-normalize" => no_normalize = true,
            "-o" | "--output" => match iter.next() {
                Some(next) => output_dir = PathBuf::from(next),
                None => return Err("--output requires a directory path".to_owned()),
            },
            other if other.starts_with('-') => {
                return Err(format!("Unknown option: {other}"));
            }
            other => {
                if input_path.is_some() {
                    return Err("Multiple input files specified".to_owned());
                }
                input_path = Some(PathBuf::from(other));
            }
        }
    }

    match input_path {
        Some(input_path) => Ok(Command::Extract {
            input_path,
            output_dir,
            no_normalize,
        }),
        None => Ok(Command::Help { code: 1 }),
    }
}

fn read_input(input_path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(input_path).map_err(|err| match err.kind() {
        ErrorKind::NotFound => format!("File not found: {}", input_path.display()),
        ErrorKind::IsADirectory => format!("Not a file: {}", input_path.display()),
        _ => err.to_string(),
    })
}

fn run(input_path: &Path, output_dir: &Path, no_normalize: bool) -> Result<(), String> {
    let data = read_input(input_path)?;

    // Version detection is informational only — extraction relies on the
    // binary's struct layout, not the version string. Never let a failed or
    // changed version banner (e.g. a future Bun format) block extraction.
    match get_executable_version(&data) {
        Ok(version) => {
            let revision = if version.revision.is_empty() {
                String::new()
            } else {
                format!(" ({})", version.revision)
            };
            println!("Bun v{}{revision}", version.version);
        }
        Err(_) => {
            eprintln!("Warning: could not determine the Bun version; continuing with extraction.");
        }
    }

    let options = ExtractOptions {
        normalize_entrypoint_file_name: !no_normalize,
    };
    let files = extract_bundled_files(&data, options).map_err(|err| err.to_string())?;

    for file in &files {
        let out_path = output_dir.join(&file.path);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&out_path, &file.contents).map_err(|err| err.to_string())?;

        if let Some(sourcemap) = &file.sourcemap {
            let json = serde_json::to_string(sourcemap).map_err(|err| err.to_string())?;
            let map_path = {
                let mut name = out_path.into_os_string();
                name.push(".map");
                PathBuf::from(name)
            };
            std::fs::write(&map_path, json).map_err(|err| err.to_string())?;
        }
    }

    println!(
        "Extracted {} file(s) to {}",
        files.len(),
        output_dir.display()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("Error: {message}");
            exit(1);
        }
    };

    match command {
        Command::Help { code } => {
            print_help();
            exit(code);
        }
        Command::Version => {
            println!("{VERSION}");
        }
        Command::Debug { input_path } => match read_input(&input_path) {
            Ok(data) => debug::run_debug(&input_path, &data),
            Err(message) => {
                eprintln!("Error: {message}");
                exit(1);
            }
        },
        Command::Extract {
            input_path,
            output_dir,
            no_normalize,
        } => {
            if let Err(message) = run(&input_path, &output_dir, no_normalize) {
                eprintln!("Error: {message}");
                exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        parse_args(&args)
    }

    #[test]
    fn parse_args_with_input_and_defaults_returns_extract() {
        assert_eq!(
            parse(&["./my-app"]).unwrap(),
            Command::Extract {
                input_path: PathBuf::from("./my-app"),
                output_dir: PathBuf::from("./decompiled"),
                no_normalize: false,
            },
        );
    }

    #[test]
    fn parse_args_with_all_options_returns_extract() {
        assert_eq!(
            parse(&["-o", "out", "--no-normalize", "app"]).unwrap(),
            Command::Extract {
                input_path: PathBuf::from("app"),
                output_dir: PathBuf::from("out"),
                no_normalize: true,
            },
        );
    }

    #[test]
    fn parse_args_with_no_input_returns_help_with_exit_1() {
        assert_eq!(parse(&[]).unwrap(), Command::Help { code: 1 });
    }

    #[test]
    fn parse_args_with_help_flag_wins_over_everything() {
        assert_eq!(parse(&["app", "-h"]).unwrap(), Command::Help { code: 0 });
    }

    #[test]
    fn parse_args_with_version_flag_returns_version() {
        assert_eq!(parse(&["-v"]).unwrap(), Command::Version);
    }

    #[test]
    fn parse_args_debug_subcommand_takes_a_binary_path() {
        assert_eq!(
            parse(&["debug", "app"]).unwrap(),
            Command::Debug {
                input_path: PathBuf::from("app"),
            },
        );
        assert_eq!(
            parse(&["debug"]).unwrap_err(),
            "debug requires a binary path"
        );
    }

    #[test]
    fn parse_args_debug_is_only_a_leading_subcommand() {
        // A file literally named "debug" still extracts when not first.
        assert!(matches!(
            parse(&["-o", "out", "debug"]).unwrap(),
            Command::Extract { .. },
        ));
    }

    #[test]
    fn parse_args_error_messages_match_the_ts_cli() {
        assert_eq!(
            parse(&["a", "b"]).unwrap_err(),
            "Multiple input files specified",
        );
        assert_eq!(parse(&["--wat"]).unwrap_err(), "Unknown option: --wat");
        assert_eq!(
            parse(&["app", "-o"]).unwrap_err(),
            "--output requires a directory path",
        );
    }
}
