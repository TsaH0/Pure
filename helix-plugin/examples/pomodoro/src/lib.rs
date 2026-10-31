//! Example plugin: a pomodoro timer (API 0.3 timers). `:pomodoro/start`
//! starts 25 minutes of work, then 5 of rest; the `plugins` statusline
//! element shows what's left, and a popup says when time is up.
//! `:pomodoro/stop` stops it. While stopped, nothing runs.
//!
//! ```toml
//! [pomodoro]
//! work = 25   # minutes
//! rest = 5
//!
//! # ~/.config/pure/config.toml: show it
//! [editor.statusline]
//! right = ["plugins", "diagnostics", "selections", "position"]
//! ```

use std::cell::Cell;

use helix_plugin_api::{
    decorations, editor, register_plugin, CommandInfo, Plugin, Registration, RequestId,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Work,
    Rest,
}

thread_local! {
    static WORK: Cell<u32> = const { Cell::new(25) };
    static REST: Cell<u32> = const { Cell::new(5) };
    /// The running phase, minutes left in it, and the pending timer.
    static RUNNING: Cell<Option<(Phase, u32, RequestId)>> = const { Cell::new(None) };
}

fn label(phase: Phase, minutes: u32) -> String {
    match phase {
        Phase::Work => format!("🍅 {minutes}m"),
        Phase::Rest => format!("☕ {minutes}m"),
    }
}

/// Show `minutes` left of `phase` and wake up in a minute.
fn run(phase: Phase, minutes: u32) -> Result<(), String> {
    decorations::set_statusline("timer", &label(phase, minutes))?;
    let id = editor::set_timer(60_000, tick)?;
    RUNNING.set(Some((phase, minutes, id)));
    Ok(())
}

fn tick() -> Result<(), String> {
    let Some((phase, minutes, _)) = RUNNING.get() else {
        return Ok(());
    };
    if minutes > 1 {
        return run(phase, minutes - 1);
    }
    let (next, message) = match phase {
        Phase::Work => (Phase::Rest, "Time for a break ☕"),
        Phase::Rest => (Phase::Work, "Back to work 🍅"),
    };
    editor::popup(&format!("**{message}**")).map(|_| ())?;
    run(next, if next == Phase::Work { WORK.get() } else { REST.get() })
}

fn minutes(config: &str, key: &str) -> Option<u32> {
    config
        .split(&format!("\"{key}\":"))
        .nth(1)?
        .split([',', '}'])
        .next()?
        .trim()
        .parse()
        .ok()
        .filter(|m| *m > 0)
}

struct Pomodoro;

impl Plugin for Pomodoro {
    fn init(config: String) -> Result<Registration, String> {
        if let Some(work) = minutes(&config, "work") {
            WORK.set(work);
        }
        if let Some(rest) = minutes(&config, "rest") {
            REST.set(rest);
        }
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("start", "Start a pomodoro: work, then rest"),
                command("stop", "Stop the pomodoro timer"),
            ],
            events: Vec::new(),
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        if let Some((_, _, id)) = RUNNING.take() {
            editor::cancel_timer(id);
        }
        match name {
            "start" => {
                editor::set_status(&format!("Pomodoro: {} minutes of work", WORK.get()));
                run(Phase::Work, WORK.get())
            }
            "stop" => {
                editor::set_status("Pomodoro stopped");
                decorations::set_statusline("timer", "")
            }
            _ => Err(format!("unknown command {name}")),
        }
    }
}

register_plugin!(Pomodoro);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn settings_and_labels() {
        assert_eq!(minutes("{\"work\":50,\"rest\":10}", "work"), Some(50));
        assert_eq!(minutes("{\"work\":0}", "work"), None);
        assert_eq!(label(Phase::Rest, 3), "☕ 3m");
    }
}
