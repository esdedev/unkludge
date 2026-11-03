//! `unkludge`, the command-line tool. Exits with 0 on success and 1 on an error, which goes to
//! stderr.

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::Parser;
use unkludge_protocol::device::PacketEvent;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error, model};

use crate::args::{Cli, Command};
use crate::commands::Failure;
use crate::output::Console;
use crate::packets::{PacketPrinter, Timing};

mod args;
mod commands;
mod dry_run;
mod output;
mod packets;
mod rk_app;

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
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), Failure> {
    if let Command::Models = cli.command {
        print_lines(output::models_table(model::supported()));
        return Ok(());
    }
    if cli.dry_run {
        run_dry(cli)
    } else {
        run_on_keyboard(cli)
    }
}

/// Opens nothing: the only device here is made of [`dry_run::DryRunTransport`]s.
fn run_dry(cli: &Cli) -> Result<(), Failure> {
    let pid = cli.pid.ok_or(
        "--dry-run needs --pid to choose a model, such as --pid 0265. `unkludge models` lists them",
    )?;
    let model = model::find(pid).ok_or(Error::Unsupported(pid))?;
    let device = dry_run::device(model)?;
    let mut err = io::stderr();
    let _ = writeln!(
        err,
        "Dry run for {} ({pid:04X}): no keyboard is opened and nothing is sent.",
        model.name
    );
    if let Command::Info | Command::Light(_) | Command::Report = cli.command {
        let _ = writeln!(
            err,
            "The config read gets a blank block, which reads as the lights being off."
        );
    }
    drive(device, cli)
}

fn run_on_keyboard(cli: &Cli) -> Result<(), Failure> {
    if rk_app::running() {
        let _ = writeln!(
            io::stderr(),
            "warning: DeviceDriver.exe, which may be RK's app, is running. It polls the keyboard \
             and sends its own settings back, so close it first"
        );
    }
    match (Device::open(cli.pid), &cli.command) {
        (Ok(device), _) => drive(device, cli),
        // Not being able to open the keyboard is just what a report should tell.
        (Err(error), Command::Report) => {
            print_lines(commands::report_header());
            print_lines([format!("Keyboard: couldn't open it: {error}")]);
            Ok(())
        }
        (Err(Error::NotFound), _) => Err(format!(
            "{}. Is it connected by USB cable? `unkludge models` lists the supported ones",
            Error::NotFound
        )
        .into()),
        (Err(error), _) => Err(error.into()),
    }
}

/// Runs the command, with whatever was asked to be seen of its packets. That is shown even if the
/// command fails.
fn drive<T: Transport>(mut device: Device<T>, cli: &Cli) -> Result<(), Failure> {
    let printer = cli
        .dry_run
        .then(|| PacketPrinter::new(cli.verbose, device.model().has_second_collection()));
    let observer = Arc::new(Mutex::new(Observer {
        printer,
        timing: cli.timing.then(Timing::default),
    }));
    let hook = Arc::clone(&observer);
    device.on_packet = Some(Box::new(move |event| {
        if let Ok(mut observer) = hook.lock() {
            observer.on_event(event);
        }
    }));

    let (mut out, mut err) = (io::stdout(), io::stderr());
    let mut console = Console {
        out: &mut out,
        err: &mut err,
    };
    let result = commands::run(&mut device, &cli.command, cli.dry_run, &mut console);
    if let Ok(mut observer) = observer.lock() {
        observer.finish();
    }
    result
}

/// What's shown of the packets as they go by.
struct Observer {
    printer: Option<PacketPrinter>,
    timing: Option<Timing>,
}

impl Observer {
    fn on_event(&mut self, event: PacketEvent) {
        match event {
            PacketEvent::Sent { collection, report } => {
                if let Some(printer) = &mut self.printer {
                    print_lines(printer.on_sent(collection, report));
                }
            }
            PacketEvent::Ack {
                accepted, latency, ..
            } => {
                if let Some(timing) = &mut self.timing {
                    timing.on_ack(accepted, latency);
                }
            }
        }
    }

    fn finish(&mut self) {
        if let Some(printer) = &mut self.printer {
            print_lines(printer.finish());
        }
        if let Some(timing) = &self.timing {
            print_lines([timing.summary()]);
            if self.printer.is_some() && timing.has_acks() {
                print_lines([
                    "A dry run makes up its acks, so this says nothing about a keyboard."
                        .to_string(),
                ]);
            }
        }
    }
}

/// Output that can't be written, as when a pipe has closed, is no reason to stop talking to a
/// keyboard halfway through.
fn print_lines(lines: impl IntoIterator<Item = String>) {
    let mut out = io::stdout();
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
}
