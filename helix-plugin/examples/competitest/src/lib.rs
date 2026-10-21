//! Example plugin: competitive programming testcases, like competitest.nvim.
//!
//! Testcases live next to the solution, as competitest.nvim stores them:
//! `sol.cpp` has `sol_input0.txt` / `sol_output0.txt`, `sol_input1.txt`...
//!
//! - `run` (`space u r`) saves nothing for you: it compiles the current file
//!   (C, C++, Rust, Go; Python and others just run), runs it on every
//!   testcase and shows the verdicts in a panel below the editor. Enter on
//!   a testcase shows its input, expected and actual output side by side.
//! - `add` (`space u a`) makes the next testcase's two files and opens them.
//! - `panel` (`space u p`) shows or hides the results.
//!
//! It runs your compiler and your program, so it needs (plugins.toml):
//!
//! ```toml
//! [competitest]
//! allow-process = true
//! allow-files = "write"          # to create testcases
//! timeout = 2000                 # ms per testcase
//! # $(FNAME) is the file, $(FNOEXT) without its extension:
//! compile.cpp = ["g++", "-O2", "-std=c++17", "-o", "$(FNOEXT)", "$(FNAME)"]
//! run.cpp = ["./$(FNOEXT)"]
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use helix_plugin_api::{
    editor,
    panels::{self, item, on_enter, Dock},
    register_plugin, CommandInfo, OpenAction, Output, Plugin, Registration, SpawnOptions,
};

/// What happened to one testcase.
#[derive(Debug, Clone, PartialEq)]
enum Verdict {
    Waiting,
    Running,
    Accepted { ms: u128 },
    WrongAnswer { ms: u128 },
    TimeLimit,
    RuntimeError(String),
}

#[derive(Debug, Clone)]
struct Testcase {
    number: u32,
    input: PathBuf,
    expected: PathBuf,
    verdict: Verdict,
    stdout: String,
    stderr: String,
}

#[derive(Default)]
struct State {
    /// The solution being tested.
    file: Option<PathBuf>,
    testcases: Vec<Testcase>,
    /// The compiler's complaint, if it failed.
    compile_error: Option<String>,
    shown: bool,
    timeout: u32,
    /// Per extension: (compile, run), with `$(FNAME)` / `$(FNOEXT)`.
    commands: Vec<(String, Option<Vec<String>>, Vec<String>)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn default_commands() -> Vec<(String, Option<Vec<String>>, Vec<String>)> {
    let v = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    vec![
        (
            "cpp".into(),
            Some(v(&["g++", "-O2", "-std=c++17", "-o", "$(FNOEXT)", "$(FNAME)"])),
            v(&["./$(FNOEXT)"]),
        ),
        (
            "c".into(),
            Some(v(&["gcc", "-O2", "-o", "$(FNOEXT)", "$(FNAME)", "-lm"])),
            v(&["./$(FNOEXT)"]),
        ),
        (
            "rs".into(),
            Some(v(&["rustc", "-O", "-o", "$(FNOEXT)", "$(FNAME)"])),
            v(&["./$(FNOEXT)"]),
        ),
        (
            "go".into(),
            Some(v(&["go", "build", "-o", "$(FNOEXT)", "$(FNAME)"])),
            v(&["./$(FNOEXT)"]),
        ),
        ("py".into(), None, v(&["python3", "$(FNAME)"])),
        ("js".into(), None, v(&["node", "$(FNAME)"])),
    ]
}

/// Settings: `timeout`, and `compile.<ext>` / `run.<ext>` argument lists.
fn parse_settings(config: &str) -> Result<(u32, Vec<(String, Option<Vec<String>>, Vec<String>)>), String> {
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|err| format!("invalid settings: {err}"))?;
    let timeout = value
        .get("timeout")
        .and_then(|t| t.as_u64())
        .map_or(2000, |t| t.clamp(100, 600_000) as u32);
    let mut commands = default_commands();
    let args = |v: &serde_json::Value, what: &str| -> Result<Vec<String>, String> {
        v.as_array()
            .ok_or_else(|| format!("{what} must be a list of strings"))?
            .iter()
            .map(|a| a.as_str().map(str::to_string).ok_or_else(|| format!("{what} must be a list of strings")))
            .collect()
    };
    for (key, compile) in [("compile", true), ("run", false)] {
        let Some(table) = value.get(key).and_then(|t| t.as_object()) else {
            continue;
        };
        for (ext, list) in table {
            let list = args(list, &format!("{key}.{ext}"))?;
            match commands.iter_mut().find(|(e, ..)| e == ext) {
                Some(entry) if compile => entry.1 = Some(list),
                Some(entry) => entry.2 = list,
                None if compile => commands.push((ext.clone(), Some(list), Vec::new())),
                None => commands.push((ext.clone(), None, list)),
            }
        }
    }
    Ok((timeout, commands))
}

/// `$(FNAME)` and `$(FNOEXT)` filled in for `file`.
fn expand(args: &[String], file: &Path) -> Vec<String> {
    let name = file.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let stem = file.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    args.iter()
        .map(|arg| arg.replace("$(FNAME)", &name).replace("$(FNOEXT)", &stem))
        .collect()
}

/// The testcases of `file`, by number.
fn find_testcases(file: &Path) -> Vec<Testcase> {
    let (Some(dir), Some(stem)) = (file.parent(), file.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    let mut found = Vec::new();
    for number in 0..1000u32 {
        let input = dir.join(format!("{stem}_input{number}.txt"));
        let expected = dir.join(format!("{stem}_output{number}.txt"));
        if !input.exists() {
            if number > 0 && !dir.join(format!("{stem}_input{}.txt", number + 1)).exists() {
                break;
            }
            continue;
        }
        found.push(Testcase {
            number,
            input,
            expected,
            verdict: Verdict::Waiting,
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    found
}

/// Whether `got` matches `expected`, ignoring trailing spaces and lines.
fn same_output(got: &str, expected: &str) -> bool {
    let lines = |s: &str| -> Vec<String> {
        let mut lines: Vec<String> = s.lines().map(|l| l.trim_end().to_string()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines
    };
    lines(got) == lines(expected)
}

fn verdict_text(verdict: &Verdict) -> (String, &'static str) {
    match verdict {
        Verdict::Waiting => ("·  waiting".into(), "comment"),
        Verdict::Running => ("…  running".into(), "comment"),
        Verdict::Accepted { ms } => (format!("✔  accepted   {ms} ms"), "diff.plus"),
        Verdict::WrongAnswer { ms } => (format!("✘  wrong answer   {ms} ms"), "error"),
        Verdict::TimeLimit => ("⏱  time limit exceeded".into(), "warning"),
        Verdict::RuntimeError(why) => (format!("✖  runtime error ({why})"), "error"),
    }
}

/// Show the results in the bottom panel.
fn show_panel() -> Result<(), String> {
    let (title, items) = STATE.with_borrow(|state| {
        let name = state
            .file
            .as_ref()
            .and_then(|f| f.file_name())
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let passed = state
            .testcases
            .iter()
            .filter(|t| matches!(t.verdict, Verdict::Accepted { .. }))
            .count();
        let title = format!("CompetiTest: {name}  {passed}/{} passed", state.testcases.len());
        let mut items = Vec::new();
        if let Some(error) = &state.compile_error {
            items.push(item("✖  compilation failed (enter: details)", "error", 0));
            let first = error.lines().next().unwrap_or_default().to_string();
            items.push(item(first, "comment", 1));
            items[0] = on_enter(items[0].clone(), "show", vec!["compile".into()]);
        }
        if state.testcases.is_empty() && state.compile_error.is_none() {
            items.push(item("no testcases: space u a adds one", "comment", 0));
        }
        for testcase in &state.testcases {
            let (text, scope) = verdict_text(&testcase.verdict);
            items.push(on_enter(
                item(format!("TC {:<3} {text}", testcase.number), scope, 0),
                "show",
                vec![testcase.number.to_string()],
            ));
        }
        (title, items)
    });
    STATE.with_borrow_mut(|state| state.shown = true);
    panels::show(&panels::panel(Dock::Bottom, &title, items, 10))
}

fn current_file() -> Result<PathBuf, String> {
    let doc = editor::current_document().ok_or("no document")?;
    let path = editor::document_path(doc)?.ok_or("the current document has no file")?;
    Ok(PathBuf::from(path))
}

/// The (compile, run) commands for `file`'s extension.
fn commands_for(file: &Path) -> Result<(Option<Vec<String>>, Vec<String>), String> {
    let ext = file.extension().map_or(String::new(), |e| e.to_string_lossy().into_owned());
    STATE.with_borrow(|state| {
        state
            .commands
            .iter()
            .find(|(e, ..)| *e == ext)
            .map(|(_, compile, run)| (compile.as_ref().map(|c| expand(c, file)), expand(run, file)))
            .ok_or_else(|| format!("no command to run .{ext} files: set run.{ext} in plugins.toml"))
    })
}

fn run() -> Result<(), String> {
    let file = current_file()?;
    let (compile, run) = commands_for(&file)?;
    let cwd = file.parent().map(|d| d.to_string_lossy().into_owned());
    STATE.with_borrow_mut(|state| {
        state.file = Some(file.clone());
        state.testcases = find_testcases(&file);
        state.compile_error = None;
        for testcase in &mut state.testcases {
            testcase.verdict = Verdict::Waiting;
        }
    });
    show_panel()?;
    match compile {
        Some(compile) => {
            editor::set_status("CompetiTest: compiling…");
            let options = SpawnOptions {
                cwd: cwd.clone(),
                timeout_ms: Some(60_000),
                ..Default::default()
            };
            editor::spawn(&compile, options, move |result| {
                match result {
                    Ok(output) if output.exit_code == Some(0) => {
                        editor::set_status("CompetiTest: running…");
                        run_from(0, run, cwd)
                    }
                    Ok(output) => {
                        let error = if output.stderr.trim().is_empty() { output.stdout } else { output.stderr };
                        STATE.with_borrow_mut(|state| state.compile_error = Some(error));
                        editor::set_error("CompetiTest: compilation failed");
                        show_panel()
                    }
                    Err(err) => {
                        STATE.with_borrow_mut(|state| state.compile_error = Some(err.clone()));
                        show_panel()?;
                        Err(err)
                    }
                }
            })?;
        }
        None => run_from(0, run, cwd)?,
    }
    Ok(())
}

/// Run testcase `index` (of the list), then the next, one at a time so the
/// times are fair.
fn run_from(index: usize, argv: Vec<String>, cwd: Option<String>) -> Result<(), String> {
    let Some(input) = STATE.with_borrow_mut(|state| {
        let testcase = state.testcases.get_mut(index)?;
        testcase.verdict = Verdict::Running;
        Some(testcase.input.clone())
    }) else {
        let summary = STATE.with_borrow(|state| {
            let passed = state
                .testcases
                .iter()
                .filter(|t| matches!(t.verdict, Verdict::Accepted { .. }))
                .count();
            format!("CompetiTest: {passed}/{} passed", state.testcases.len())
        });
        editor::set_status(&summary);
        return show_panel();
    };
    show_panel()?;
    let stdin = std::fs::read_to_string(&input).map_err(|err| format!("{}: {err}", input.display()))?;
    let timeout = STATE.with_borrow(|state| state.timeout);
    let options = SpawnOptions {
        cwd: cwd.clone(),
        stdin: Some(stdin),
        timeout_ms: Some(timeout),
    };
    let started = Instant::now();
    let next = argv.clone();
    editor::spawn(&argv, options, move |result| {
        let ms = started.elapsed().as_millis();
        STATE.with_borrow_mut(|state| {
            let Some(testcase) = state.testcases.get_mut(index) else {
                return;
            };
            let expected = std::fs::read_to_string(&testcase.expected).unwrap_or_default();
            testcase.verdict = judge(&result, &expected, ms, timeout);
            if let Ok(output) = &result {
                testcase.stdout = output.stdout.clone();
                testcase.stderr = output.stderr.clone();
            } else if let Err(err) = &result {
                testcase.stderr = err.clone();
            }
        });
        run_from(index + 1, next, cwd)
    })?;
    Ok(())
}

fn judge(result: &Result<Output, String>, expected: &str, ms: u128, timeout: u32) -> Verdict {
    match result {
        Err(err) if err.contains("timed out") || err.contains("timeout") => Verdict::TimeLimit,
        Err(err) => Verdict::RuntimeError(err.clone()),
        Ok(output) if ms >= timeout as u128 || output.stderr.contains("[killed after") => {
            Verdict::TimeLimit
        }
        Ok(output) if output.exit_code != Some(0) => Verdict::RuntimeError(match output.exit_code {
            Some(code) => format!("exit {code}"),
            None => "killed".into(),
        }),
        Ok(output) if same_output(&output.stdout, expected) => Verdict::Accepted { ms },
        Ok(_) => Verdict::WrongAnswer { ms },
    }
}

/// Input, expected and actual output of testcase `which`, in a scratch
/// buffer (or the compiler's errors).
fn show(which: &str) -> Result<(), String> {
    let text = STATE.with_borrow(|state| {
        if which == "compile" {
            return state.compile_error.clone();
        }
        let number: u32 = which.parse().ok()?;
        let testcase = state.testcases.iter().find(|t| t.number == number)?;
        let input = std::fs::read_to_string(&testcase.input).unwrap_or_default();
        let expected = std::fs::read_to_string(&testcase.expected).unwrap_or_default();
        let (verdict, _) = verdict_text(&testcase.verdict);
        let mut text = format!("Testcase {number}: {verdict}\n\n── Input ──\n{input}");
        text.push_str(&format!("\n── Expected ──\n{expected}\n── Got ──\n{}", testcase.stdout));
        if !testcase.stderr.trim().is_empty() {
            text.push_str(&format!("\n── Stderr ──\n{}", testcase.stderr));
        }
        Some(text)
    });
    let text = text.ok_or("no such testcase: run first")?;
    editor::open_scratch(&text, None, OpenAction::Vsplit)
}

/// Make the next testcase's files and open them.
fn add() -> Result<(), String> {
    let file = current_file()?;
    let (dir, stem) = (
        file.parent().ok_or("no directory")?,
        file.file_stem().ok_or("no file name")?.to_string_lossy().into_owned(),
    );
    let number = (0..1000)
        .find(|n| !dir.join(format!("{stem}_input{n}.txt")).exists())
        .ok_or("too many testcases")?;
    let input = dir.join(format!("{stem}_input{number}.txt"));
    let output = dir.join(format!("{stem}_output{number}.txt"));
    for path in [&input, &output] {
        std::fs::write(path, "").map_err(|err| {
            format!("{}: {err} (allow-files = \"write\" in plugins.toml)", path.display())
        })?;
    }
    editor::open_file_at(&input.to_string_lossy(), OpenAction::Vsplit, None, None)?;
    editor::open_file_at(&output.to_string_lossy(), OpenAction::Hsplit, None, None)?;
    editor::set_status(&format!("CompetiTest: testcase {number}: input above, expected output below"));
    Ok(())
}

struct CompetiTest;

impl Plugin for CompetiTest {
    fn init(config: String) -> Result<Registration, String> {
        let (timeout, commands) = parse_settings(&config)?;
        STATE.with_borrow_mut(|state| {
            state.timeout = timeout;
            state.commands = commands;
        });
        let command = |name: &str, doc: &str| CommandInfo {
            name: name.into(),
            doc: doc.into(),
        };
        Ok(Registration {
            commands: vec![
                command("run", "Compile and run the current file on its testcases"),
                command("add", "Add a testcase for the current file"),
                command("panel", "Show or hide the testcase results"),
                command("show", "Show a testcase's input, expected and actual output"),
            ],
            events: Vec::new(),
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "run" => run(),
            "add" => add(),
            "panel" => {
                if STATE.with_borrow(|state| state.shown) {
                    STATE.with_borrow_mut(|state| state.shown = false);
                    panels::close(Dock::Bottom);
                    Ok(())
                } else {
                    show_panel()
                }
            }
            "show" => show(args.first().map_or("0", String::as_str)),
            _ => Err(format!("unknown command {name}")),
        }
    }
}

register_plugin!(CompetiTest);

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn outputs_compare_without_trailing_space() {
        assert!(same_output("1 2\n3\n\n", "1 2   \n3"));
        assert!(!same_output("1 2\n", "1 3\n"));
    }

    #[test]
    fn verdicts() {
        let ok = |code, out: &str| Ok(Output { exit_code: code, stdout: out.into(), stderr: String::new() });
        assert_eq!(judge(&ok(Some(0), "5\n"), "5", 3, 1000), Verdict::Accepted { ms: 3 });
        assert_eq!(judge(&ok(Some(0), "4\n"), "5", 3, 1000), Verdict::WrongAnswer { ms: 3 });
        assert_eq!(judge(&ok(Some(1), ""), "5", 3, 1000), Verdict::RuntimeError("exit 1".into()));
        let killed = Ok(Output { exit_code: None, stdout: String::new(), stderr: "[killed after 1000 ms]".into() });
        assert_eq!(judge(&killed, "5", 999, 1000), Verdict::TimeLimit);
    }

    #[test]
    fn settings_override_commands() {
        let (timeout, commands) = parse_settings(
            r#"{"timeout": 500, "run": {"py": ["pypy3", "$(FNAME)"]}, "compile": {"kt": ["kotlinc", "$(FNAME)"]}}"#,
        )
        .unwrap();
        assert_eq!(timeout, 500);
        let py = commands.iter().find(|(e, ..)| e == "py").unwrap();
        assert_eq!(py.2, ["pypy3", "$(FNAME)"]);
        assert!(commands.iter().any(|(e, c, _)| e == "kt" && c.is_some()));
        assert_eq!(expand(&py.2, Path::new("/x/sol.py")), ["pypy3", "sol.py"]);
        assert!(parse_settings(r#"{"run": {"py": "nope"}}"#).is_err());
    }
}
