mod api;
mod auth;
mod commands;
mod config;
mod cred;
mod error;
mod fsproj;
mod hjson;
mod http;
mod ignore;
mod lz4;
mod model;
mod project;
mod rbxl;
mod rbxlx;
mod ui;
mod upload;
mod util;
mod validate;
mod xml;

use clap::{Parser, Subcommand};
use error::CliResult;

/// Stewdio: a standalone Roblox place file decompiler, project manager, and compiler.
#[derive(Parser)]
#[command(name = "stewdio", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new Stewdio project
    New(commands::NewArgs),
    /// Initialize an existing directory
    Init(commands::InitArgs),
    /// Configure Stewdio settings
    Config(config::ConfigArgs),
    /// Manage Roblox authentication
    Auth(auth::AuthArgs),
    /// Compile a Stewdio project
    Compile(commands::CompileArgs),
    /// Decompile a Roblox place or model file
    Decompile(commands::DecompileArgs),
    /// Upload a compiled place to Roblox
    Upload(upload::UploadArgs),
    /// Validate a Stewdio project
    Validate(commands::ValidateArgs),
    /// Display project information
    Info,
    /// Manage the cached Roblox API database used for validation and autofill
    Api(api::ApiArgs),
    /// Display Stewdio version
    Version,
}

fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::New(a) => commands::new_project(a),
        Command::Init(a) => commands::init_project(a),
        Command::Config(a) => config::run(a),
        Command::Auth(a) => auth::run(a),
        Command::Compile(a) => commands::compile(a),
        Command::Decompile(a) => commands::decompile(a),
        Command::Upload(a) => upload::run(a),
        Command::Validate(a) => commands::validate_project(a),
        Command::Info => commands::info(),
        Command::Api(a) => api::run(a),
        Command::Version => {
            println!("stewdio {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn main() {
    let cli = Cli::parse();
    // Deeply nested Roblox hierarchies are processed recursively; give the worker a big stack.
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run(cli))
        .expect("failed to spawn worker thread");
    let code = match handle.join() {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            ui::error(&e.message);
            e.code
        }
        Err(_) => {
            ui::error("internal error");
            1
        }
    };
    std::process::exit(code);
}
