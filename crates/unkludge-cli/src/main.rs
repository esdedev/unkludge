//! `unkludge`, the command-line tool. Exits with 0 on success and 1 on an error, which goes to
//! stderr. An upload that the keyboard took but didn't confirm the end of exits with 2, with a warning
//! on stderr, as it's neither of those.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::Parser;
use unkludge_protocol::device::list_collections;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error, model};
use unkludge_shared::{device_lock, report_lines, rk_app};

use crate::args::{Cli, Command, KeyboardCommand};
use crate::commands::{Failure, Options, Unconfirmed};
use crate::output::Console;
use crate::packets::Observer;

mod args;
mod commands;
mod dry_run;
mod interrupt;
mod output;
mod packets;

/// See the top of this file.
const EXIT_UNCONFIRMED: u8 = 2;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    let (line, code) = ending(&run(&cli));
    if let Some(line) = line {
        let _ = writeln!(io::stderr(), "{line}");
    }
    ExitCode::from(code)
}

/// What to say last, and the exit code.
fn ending(result: &Result<(), Failure>) -> (Option<String>, u8) {
    match result {
        Ok(()) => (None, 0),
        Err(error) if error.is::<Unconfirmed>() => {
            (Some(format!("warning: {error}")), EXIT_UNCONFIRMED)
        }
        Err(error) => (Some(format!("error: {error}")), 1),
    }
}

fn run(cli: &Cli) -> Result<(), Failure> {
    cli.check()?;
    match &cli.command {
        Command::Models(args) => {
            let lines = if args.markdown {
                output::models_markdown(model::supported())
            } else {
                output::models_table(model::supported())
            };
            Console::with_stdio(|console| console.lines(lines));
            Ok(())
        }
        Command::Keyboard(command) if cli.dry_run => run_dry(cli, command),
        Command::Keyboard(command) => run_on_keyboard(cli, command),
    }
}

/// Opens nothing: the only device here is made of [`dry_run::DryRunTransport`]s.
fn run_dry(cli: &Cli, command: &KeyboardCommand) -> Result<(), Failure> {
    let pid = cli.pid.ok_or(
        "--dry-run needs --pid to choose a model, such as --pid 0265. `unkludge models` lists them",
    )?;
    let model = model::find(pid).ok_or(Error::Unsupported(pid))?;
    let device = dry_run::device(model)?;
    Console::with_stdio(|console| {
        console.lines_err(commands::dry_run_notes(model, cli.timing));
    });
    let host = ["Collections: not listed in a dry run".to_string()];
    drive(device, cli, command, &host)
}

fn run_on_keyboard(cli: &Cli, command: &KeyboardCommand) -> Result<(), Failure> {
    let rk_app_running = rk_app::running();
    Console::with_stdio(|console| commands::warn_if_rk_app(rk_app_running, console));

    // Only a report needs this up front, and asking the system sends nothing to a keyboard.
    let host = match command {
        KeyboardCommand::Report => report_lines::host_lines(rk_app_running, &list_collections()),
        _ => Vec::new(),
    };
    match device_lock::open_locked(device_lock::COMMAND_WAIT, || Device::open(cli.pid)) {
        Ok(device) => {
            if let KeyboardCommand::Screen(_) = command {
                interrupt::install();
            }
            drive(device, cli, command, &host)
        }
        // Not being able to open the keyboard is just what a report should tell.
        Err(error) if matches!(command, KeyboardCommand::Report) => {
            Console::with_stdio(|console| {
                console.lines(report_lines::report_header());
                console.lines(&host);
                console.line(format_args!("Keyboard: couldn't open it: {error}"));
            });
            Ok(())
        }
        Err(error) => {
            let linux = cfg!(target_os = "linux");
            Err(commands::open_failure(&error, cli.pid, linux).into())
        }
    }
}

/// Runs the command, with whatever was asked to be seen of its packets. That is shown even if the
/// command fails or gets cancelled.
fn drive<T: Transport>(
    mut device: Device<T>,
    cli: &Cli,
    command: &KeyboardCommand,
    host: &[String],
) -> Result<(), Failure> {
    let two_collections = device.model().has_second_collection();
    let observer = Arc::new(Mutex::new(Observer::new(
        cli.dry_run,
        cli.timing,
        cli.verbose,
        two_collections,
    )));
    let hook = Arc::clone(&observer);
    device.on_packet = Some(Box::new(move |event| {
        let at = Instant::now();
        if let Ok(mut observer) = hook.lock() {
            Console::with_stdio(|console| observer.on_event(event, at, console));
        }
    }));

    let options = Options {
        dry_run: cli.dry_run,
        terminal: io::stderr().is_terminal(),
        cancel: interrupt::flag(),
        host,
    };
    let result =
        Console::with_stdio(|console| commands::run(&mut device, command, &options, console));
    interrupt::finished();
    if let Ok(mut observer) = observer.lock() {
        Console::with_stdio(|console| observer.finish(console));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_way_of_ending_has_its_exit_code() {
        assert_eq!(ending(&Ok(())), (None, 0));
        let (line, code) = ending(&Err("it broke".into()));
        assert_eq!(line.as_deref(), Some("error: it broke"));
        assert_eq!(code, 1);
    }

    #[test]
    fn an_upload_nobody_confirmed_is_neither_a_success_nor_an_error() {
        let (line, code) = ending(&Err(Unconfirmed.into()));
        let line = line.unwrap();
        assert!(line.starts_with("warning: "), "{line}");
        assert!(line.contains("finish"), "{line}");
        assert_eq!(code, 2);
    }
}
