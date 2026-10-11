//! forestui — a terminal UI for managing Git worktrees.

mod app;
mod cli;
mod event;
mod fuzzy;
mod modal;
mod models;
mod picker;
mod prompt;
mod prune;
mod repo_settings;
mod services;
mod state;
mod terminal;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;
mod util;
mod version_check;
mod work;

use app::App;
use clap::Parser;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    // A one-shot maintenance action: report and exit, before anything
    // re-executes into tmux or paints a UI.
    if let Some(action) = args.claude_plugin {
        return run_claude_plugin_action(action);
    }
    if let Some(cli::Command::Prune { forest_path }) = &args.command {
        return run_prune(forest_path.as_deref());
    }

    // Re-executes into tmux and never returns when we are outside one.
    cli::ensure_tmux(&args)?;

    // Running from source always behaves as dev mode, as the Textual build did.
    let dev_mode = args.dev_mode || cli::VERSION == "0.0.0";
    services::tmux::rename_window(&cli::window_name(dev_mode));

    services::settings::set_forest_path(args.forest_path.as_deref());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = runtime.block_on(run(!args.no_self_update, args.input_modes()));

    if let Err(error) = &result {
        report_crash(error);
    }
    result
}

/// `--claude-plugin status|install|uninstall`.
///
/// Prints the plugin's directory and every file involved before doing anything,
/// so this is also the way to see exactly what an install writes without
/// running one.
fn run_claude_plugin_action(action: cli::ClaudePluginAction) -> anyhow::Result<()> {
    use services::claude_plugin::{self, Status};
    use std::fmt::Write as _;

    let dir = claude_plugin::plugin_dir();
    let mut out = String::new();
    let _ = writeln!(out, "plugin:  {}", claude_plugin::PLUGIN_NAME);
    let _ = writeln!(out, "path:    {}", dir.display());
    let _ = writeln!(out, "status:  {}", claude_plugin::status().label());

    let result = match action {
        cli::ClaudePluginAction::Status => {
            if let Status::Drifted(files) = claude_plugin::status() {
                for file in files {
                    let _ = writeln!(out, "modified: {file}");
                }
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "An install writes only these files. Your settings.json is not touched;"
            );
            let _ = writeln!(out, "Claude discovers plugin directories on its own.");
            for path in claude_plugin::planned_paths() {
                let _ = writeln!(out, "  {}", path.display());
            }
            Ok(())
        }
        cli::ClaudePluginAction::Install => claude_plugin::install(false).map(|()| {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Installed. It takes effect in Claude sessions started from now on."
            );
            let _ = writeln!(
                out,
                "Renaming a forestui tab now renames the Claude session in it."
            );
            let _ = writeln!(
                out,
                "To stop that, uninstall — there is no per-tab switch to remember."
            );
            if !claude_plugin::jq_available() {
                let _ = writeln!(out);
                let _ = writeln!(
                    out,
                    "Note: `jq` is not on PATH. The hook reads the session's own name"
                );
                let _ = writeln!(
                    out,
                    "with it, so until jq is installed the sync does nothing at all"
                );
                let _ = writeln!(out, "rather than half of it.");
            }
        }),
        cli::ClaudePluginAction::Uninstall => claude_plugin::uninstall().map(|()| {
            let _ = writeln!(out);
            let _ = writeln!(out, "Removed.");
        }),
    };

    // Written once, and a write error is dropped: piping this into `head`
    // closes the pipe, and `println!` turns that into a panic.
    let _ = std::io::stdout().write_all(out.as_bytes());
    result.map_err(|e| anyhow::anyhow!(e))
}

/// `forestui prune [forest]`: the repository pane's prune over the whole
/// forest, from a shell. Never touches tmux, so it runs anywhere — a cron
/// job, a plain terminal, a session with no tmux server at all.
fn run_prune(forest_path: Option<&str>) -> anyhow::Result<()> {
    services::settings::set_forest_path(forest_path);
    let forest = services::settings::get_forest_path();
    // `AppState::load` creates the directory, which is right for the app and
    // wrong for a mistyped path handed to a one-shot command.
    if !forest.is_dir() {
        anyhow::bail!("no forest at {}", forest.display());
    }
    let mut state = state::AppState::load();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (results, unchecked) = runtime.block_on(async {
        let results = prune::prune_forest(&mut state).await;
        (results, prune::links_unchecked(&state).await)
    });
    let summary = prune::summary(&results);
    if unchecked {
        eprintln!(
            "Warning: gh is not signed in, so worktrees linked to issues or pull requests were not checked."
        );
    }

    let mut out = summary.lines.join("\n");
    out.push('\n');
    // Dropped on error for the same reason as the plugin action's output.
    let _ = std::io::stdout().write_all(out.as_bytes());

    if let Some(error) = state.take_save_error() {
        anyhow::bail!("could not save {}: {error}", forest.display());
    }
    if summary.failed {
        std::process::exit(1);
    }
    Ok(())
}

async fn run(self_update: bool, modes: terminal::InputModes) -> anyhow::Result<()> {
    let (tx, mut rx) = event::start();
    let mut app = App::new(tx, cli::VERSION.to_string());
    app.self_update = self_update;
    app.input_modes = modes;

    // Armed before `ratatui::init()` enables raw mode, so the settings the
    // handler restores are the ones the user's shell had.
    terminal::install_signal_handlers();
    let mut ui_terminal = ratatui::init();
    // After `init`, so this hook runs ahead of ratatui's screen restore.
    terminal::chain_panic_hook();
    let mode_guard = terminal::ModeGuard::enable(std::io::stdout(), modes);
    app.on_start();

    let outcome = loop {
        // Skip the repaint when the last batch of events changed nothing on
        // screen — pointer motion inside one control is the common case.
        if app.redraw {
            app.redraw = false;
            if let Err(error) = ui_terminal.draw(|frame| ui::draw(frame, &mut app)) {
                break Err(anyhow::Error::from(error));
            }
        }
        let Some(event) = rx.recv().await else {
            break Ok(());
        };
        app.handle_event(event);

        app.drain(&mut rx);

        if app.should_quit {
            break Ok(());
        }
    };

    // Explicit on the way out so the modes are reset before the screen is,
    // matching the order every other exit path produces. The guard is what
    // makes the paths that never reach this line safe.
    app.release_tmux_options();
    drop(mode_guard);
    ratatui::restore();
    outcome
}

/// Mirror the Textual build's crash handling: write a log, print it, and pause
/// so the message is readable before the tmux window closes.
fn report_crash(error: &anyhow::Error) {
    let log_path = util::home_dir().join(".forestui-error.log");
    let report = format!("{error:?}\n");
    let _ = std::fs::write(&log_path, &report);
    eprintln!("{report}");
    eprintln!("\nError: {error}");
    eprintln!("\nError log written to: {}", log_path.display());
    eprint!("Press Enter to exit...");
    let _ = std::io::stderr().flush();
    let mut discard = String::new();
    let _ = std::io::stdin().read_line(&mut discard);
}
