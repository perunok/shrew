//! shrew: a tiny, fast, keyboard-first GPU text editor with vim and Sublime keys. The binary is `srw` (`shrew` is
//! a symlink to it).
//!
//! - [`cli`]: command-line flags.
//! - [`launch`] / [`ipc`] / [`remote`]: `srw file` hands the files to the running shrew (single instance) or starts
//!   one detached from the terminal; the running one applies the requests.
//! - [`state`]: the editor plus config, syntax, background loads, updates and MCP (no window).
//! - [`app`]: the winit event loop, input and the renderer.
//! - [`draw`] / [`layout`] / [`anim`]: building a frame from the state, and the geometry hit testing shares.
//! - [`minimap`]: the Sublime-style overview column at the right of the editor.
//! - [`about`]: the About panel's text (version, the Shrew Manifesto, credits); [`open`] opens its links.
//! - [`sidebar`] / [`recent`] / `symbols`: project navigation (the file tree, recent files and projects, the symbol
//!   provider behind go to symbol).
//! - `terminal`: the built-in terminal panel (cargo feature `terminal`).
//! - [`settings`]: the Settings panel (Ctrl+,): sections of setting rows, saved to config.toml.
//! - [`statusbar`]: the status bar's clickable items and menus; [`language`]: *Set Language…* for a buffer.
//! - [`headless`]: `--screenshot`, a frame rendered without a window.
//! - [`titlebar`]: the custom title bar (tabs, menus, window buttons) and its window behaviour.
//! - [`mouse`]: what the mouse does, without a window (the app and headless runs share it); [`context_menu`]: the
//!   right-click menus every part of the UI opens; [`menus`]: the menus of the text and the gutter, and the primary
//!   selection (middle-click paste).
//! - [`tabs`]: the buffer tabs (their layout, look, mouse handling and right-click menu).
//! - [`toast`]: the one toast look (git's progress and errors, the update notifications).
//! - `updates`: in-app updates (cargo feature `update`): the schedule, the notification, the Update shrew dialog,
//!   Settings > Updates, restarting with the session restored, `--self-update`.
//! - `git`: git integration (cargo feature `git`): change markers, inline blame, the diff view, the
//!   Source Control panel, log, blame and conflicts.

mod about;
mod anim;
mod app;
mod cli;
#[cfg(feature = "clipboard")]
mod clipboard;
mod colors;
mod context_menu;
mod draw;
#[cfg(feature = "git")]
mod git;
mod headless;
mod ipc;
mod keys;
mod language;
mod launch;
mod layout;
// ----- editor menus -----
mod menus;
// -----
mod minimap;
mod mouse;
mod open;
mod recent;
mod remote;
mod settings;
mod sidebar;
mod state;
mod statusbar;
// ----- tabs -----
mod tabs;
// -----
#[cfg(feature = "syntax")]
mod symbols;
#[cfg(feature = "terminal")]
mod terminal;
mod titlebar;
#[cfg(any(feature = "git", feature = "update"))]
mod toast;
#[cfg(feature = "update")]
mod updates;

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
        Action::SelfUpdate => self_update(),
        Action::Edit => launch::run(cli),
    };
    if let Err(e) = result {
        eprintln!("{name}: {e}");
        std::process::exit(1);
    }
}

/// `--self-update`: see `updates::cli`.
fn self_update() -> Result<(), String> {
    #[cfg(feature = "update")]
    return updates::cli::run();
    #[cfg(not(feature = "update"))]
    Err("updates are not compiled into this build".into())
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
