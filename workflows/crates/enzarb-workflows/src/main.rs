use clap::Parser;
use enzarb_workflows::cli::Cli;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    enzarb_workflows::init_tls();
    enzarb_workflows::init_tracing();
    match enzarb_workflows::run_cli(Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            std::process::ExitCode::FAILURE
        }
    }
}
