//! The `onepipeline` binary.
//!
//! At the interface-only stage this parses the full command surface from
//! `docs/contract.md` and then refuses: no run starts, no view reports, and no
//! subcommand does its work. The refusal is loud and carries an exit code the
//! contract has not spent, so a caller that wired this in early fails visibly
//! rather than reading an empty stream as a run that settled.

use clap::Parser;
use onepipeline::cli::{ChannelCommand, Cli, Command, RoundCommand};
use onepipeline::error::EXIT_NOT_IMPLEMENTED;

fn main() {
    let cli = Cli::parse();
    let command = name_of(&cli.command);
    eprintln!(
        "onepipeline: NOT IMPLEMENTED — `{command}` parses per docs/contract.md, \
         but this build implements none of it."
    );
    eprintln!(
        "ACTION: use a release that implements the contract; \
         `onepipeline --help` shows the surface this one agrees to."
    );
    std::process::exit(EXIT_NOT_IMPLEMENTED);
}

fn name_of(command: &Command) -> &'static str {
    match command {
        Command::Start(_) => "start",
        Command::Adopt(_) => "adopt",
        Command::Round(RoundCommand::Run(_)) => "round run",
        Command::Round(RoundCommand::Next(_)) => "round next",
        Command::Channel(ChannelCommand::Serve(_)) => "channel serve",
        Command::Next(_) => "next",
        Command::Reply(_) => "reply",
        Command::Surface(_) => "surface",
        Command::Attest(_) => "attest",
        Command::Stop(_) => "stop",
        Command::Runs(_) => "runs",
        Command::Status(_) => "status",
        Command::Host => "host",
        Command::Monitor(_) => "monitor",
        Command::Results(_) => "results",
        Command::Goals(_) => "goals",
        Command::Telemetry(_) => "telemetry",
    }
}
