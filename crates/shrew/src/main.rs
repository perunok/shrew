//! shrew: a tiny, fast, keyboard-first GPU text editor with vim and Sublime keys. The binary is `srw` (`shrew` is
//! a symlink to it).
//!
//! - [`cli`]: command-line flags.
//! - [`launch`] / [`ipc`] / [`remote`]: `srw file` hands the files to the running shrew (single instance) or starts
//!   one detached from the terminal; the running one applies the requests.
//! - [`state`]: the editor plus config, syntax, background loads, updates and MCP (no window).
//! - [`app`]: the winit event loop, input and the renderer.
//! - [`draw`] / [`layout`] / [`anim`]: building a frame from the state, and the geometry hit testing shares.
//! - [`about`]: the About panel's text (version, the Shrew Manifesto, credits).
//! - [`sidebar`] / [`recent`] / `symbols`: project navigation (the file tree, recent files and projects, the symbol
//!   provider behind go to symbol).
//! - [`headless`]: `--screenshot`, a frame rendered without a window.
//! - [`titlebar`]: the custom title bar (tabs, menus, window buttons) and its window behaviour.

mod about;
mod anim;
mod app;
mod cli;
#[cfg(feature = "clipboard")]
mod clipboard;
mod colors;
mod draw;
mod headless;
mod ipc;
mod keys;
mod launch;
mod layout;
mod recent;
mod remote;
mod sidebar;
mod state;
#[cfg(feature = "syntax")]
mod symbols;
mod titlebar;

use cli::{Action, Cli};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // the MCP bridge is launched by AI clients many times: answer before touching anything else
    if args.iter().any(|a| a == "--mcp-stdio") {
        std::process::exit(mcp_stdio());
    }
    let name = cli::program_name();
    let cli = match Cli::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{name}: {e}");
            std::process::exit(2);
        }
    };
    // the binary crate is `srw`, the library crates `shrew_*`
    let filter = if cli.stats { "srw=info,shrew=info,warn" } else { "warn" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).format_timestamp(None).init();
    let result = match &cli.action {
        Action::Version => {
            println!("shrew {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Action::Help => {
            print!("{}", cli::usage(&name));
            Ok(())
        }
        Action::PrintConfig => {
            print!("{}", shrew_config::default_config_text());
            Ok(())
        }
        Action::Features => {
            print!("{}", state::features_report(&shrew_config::load().config));
            Ok(())
        }
        Action::McpStdio => Ok(()),
        Action::Screenshot(out) => headless::run(&cli, &out.clone()),
        Action::Edit => launch::run(cli),
    };
    if let Err(e) = result {
        eprintln!("{name}: {e}");
        std::process::exit(1);
    }
}

fn mcp_stdio() -> i32 {
    #[cfg(feature = "mcp")]
    {
        match shrew_mcp::files::mcp_dir() {
            Some(dir) => shrew_mcp::bridge::run(&dir),
            None => {
                eprintln!("shrew: no config directory for the MCP bridge");
                1
            }
        }
    }
    #[cfg(not(feature = "mcp"))]
    {
        eprintln!("shrew: MCP is not compiled into this build");
        1
    }
}
