mod commands;
mod error;
mod fsproj;
mod hjson;
mod ignore;
mod model;
mod project;
mod lz4;
mod rbxl;
mod rbxlx;
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
    /// Configure Stewdio settings (not yet implemented)
    Config(commands::PassArgs),
    /// Manage Roblox authentication (not yet implemented)
    Auth(commands::PassArgs),
    /// Compile a Stewdio project
    Compile(commands::CompileArgs),
    /// Decompile a Roblox place file
    Decompile(commands::DecompileArgs),
    /// Upload a compiled place to Roblox (not yet implemented)
    Upload(commands::PassArgs),
    /// Validate a Stewdio project
    Validate(commands::ValidateArgs),
    /// Display project information
    Info,
    /// Display Stewdio version
    Version,
}

fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::New(a) => commands::new_project(a),
        Command::Init(a) => commands::init_project(a),
        Command::Config(_) => commands::not_implemented("config", "a later release"),
        Command::Auth(_) => commands::not_implemented("auth", "v0.4"),
        Command::Compile(a) => commands::compile(a),
        Command::Decompile(a) => commands::decompile(a),
        Command::Upload(_) => commands::not_implemented("upload", "v0.4"),
        Command::Validate(a) => commands::validate_project(a),
        Command::Info => commands::info(),
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
            eprintln!("[Error] {}", e.message);
            e.code
        }
        Err(_) => {
            eprintln!("[Error] internal error");
            1
        }
    };
    std::process::exit(code);
}
