mod client;
mod daemon;
mod model;
mod prompt;
mod store;
mod system;
mod tray;

use std::process::ExitCode;

const USAGE: &str = "\
Keep this computer awake for a set time, even with the lid closed.

Usage:
  insomnia                    run the tray daemon (normally started by systemd)
  insomnia start [DURATION]   keep awake for DURATION, or the default preset
  insomnia extend DURATION    add time to the running session
  insomnia end                end the session now
  insomnia status             show the session, lid, battery and held changes
  insomnia prompt [--extend]  open the Days / Hours / Minutes entry
  insomnia quit               end the session and stop the daemon

DURATION is a number of minutes, or units such as 45m, 2h30m, 1d 4h.
The longest session is 30 days.";

enum Cli {
    Daemon,
    Prompt { extend: bool },
    Start(u64),
    Extend(u64),
    End,
    Status,
    Quit,
    SimulateLid(String),
    Help,
}

fn parse(args: &[String]) -> Result<Cli, String> {
    let rest = || args[1..].join(" ");
    let one = |what: &str| match args.len() {
        1 => Ok(()),
        _ => Err(format!("`insomnia {what}` takes no arguments")),
    };
    let Some(verb) = args.first() else { return Ok(Cli::Daemon) };
    match verb.as_str() {
        "daemon" => one("daemon").map(|_| Cli::Daemon),
        "start" if args.len() == 1 => Ok(Cli::Start(0)),
        "start" => model::parse_duration(&rest()).map(Cli::Start),
        "extend" if args.len() == 1 => Err("`insomnia extend` needs a duration, such as 30m".into()),
        "extend" => model::parse_duration(&rest()).map(Cli::Extend),
        "end" => one("end").map(|_| Cli::End),
        "status" => one("status").map(|_| Cli::Status),
        "quit" => one("quit").map(|_| Cli::Quit),
        "prompt" => match &args[1..] {
            [] => Ok(Cli::Prompt { extend: false }),
            [flag] if flag == "--extend" => Ok(Cli::Prompt { extend: true }),
            _ => Err("`insomnia prompt` takes only --extend".into()),
        },
        "simulate-lid" if args.len() == 2 => Ok(Cli::SimulateLid(args[1].clone())),
        "help" | "-h" | "--help" => Ok(Cli::Help),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse(&args) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("insomnia: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let answer = match cli {
        Cli::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Cli::Daemon => {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
            return match runtime.block_on(daemon::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("insomnia: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Cli::Prompt { extend } => {
            return std::process::Termination::report(prompt::run(extend));
        }
        Cli::Start(seconds) => client::call(|p| async move { p.start(seconds).await }),
        Cli::Extend(seconds) => client::call(|p| async move { p.extend(seconds).await }),
        Cli::End => client::call(|p| async move { p.end().await }),
        Cli::Status => client::call(|p| async move { p.status().await }),
        Cli::Quit => client::call(|p| async move { p.quit().await }),
        Cli::SimulateLid(state) => client::call(|p| async move { p.simulate_lid(&state).await }),
    };
    match answer {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("insomnia: {e}");
            ExitCode::FAILURE
        }
    }
}
