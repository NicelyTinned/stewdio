mod api;
mod auth;
mod commands;
mod editor;
mod explorer;
mod diff;
mod history;
mod inspect;
mod luau;
mod settings;
mod tools;
mod tui;
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
mod remote;
mod rbxlx;
mod ui;
mod upload;
mod util;
mod validate;
mod xml;

use clap::{CommandFactory, Parser, Subcommand};
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
    /// Browse and edit instances, properties and scripts in a terminal UI
    Explorer(explorer::ExplorerArgs),
    /// View or edit Project.toml from the command line
    Settings(settings::SettingsArgs),
    /// Re-upload an earlier snapshot of the place as a new version
    Revert(history::RevertArgs),
    /// Check your environment (curl, API data, credentials, project)
    Doctor,
    /// Remove build output (and with --all, backups and history)
    Clean(tools::CleanArgs),
    /// Rebuild automatically when project files change
    Watch(tools::WatchArgs),
    /// Print a shell completion script
    Completions(CompletionsArgs),
    /// Create a new project by downloading a place from Roblox
    From(remote::FromArgs),
    /// Replace this project's instances with the latest copy of its place
    Pull(remote::PullArgs),
    /// Compare two place files or project directories
    Diff(diff::DiffArgs),
    /// Print the instance hierarchy of the project
    Tree(inspect::TreeArgs),
    /// Search instance names, script source and property values
    Find(inspect::FindArgs),
    /// Compile the project and open it with the default program (Roblox Studio)
    Open,
    /// Display Stewdio version
    Version,
}

#[derive(clap::Args)]
struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    shell: clap_complete::Shell,
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
        Command::Explorer(a) => explorer::run(a),
        Command::Settings(a) => settings::run(a),
        Command::Revert(a) => history::run(a),
        Command::Doctor => tools::doctor(),
        Command::Clean(a) => tools::clean(a),
        Command::Watch(a) => tools::watch(a),
        Command::Completions(a) => {
            let mut cmd = Cli::command();
            clap_complete::generate(a.shell, &mut cmd, "stewdio", &mut std::io::stdout());
            Ok(())
        }
        Command::From(a) => remote::from(a),
        Command::Pull(a) => remote::pull(a),
        Command::Diff(a) => diff::run(a),
        Command::Tree(a) => inspect::tree(a),
        Command::Find(a) => inspect::find(a),
        Command::Open => commands::open_built(),
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
