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
//! - `receive` (`space u t`) gets testcases from the Competitive Companion
//!   browser extension: click its button on a problem page and the
//!   problem's tests replace the current file's testcases.
//!   `receive problem` makes `<cwd>/<Problem>.<ext>` from a template
//!   instead and opens it, `receive contest` puts every problem of a
//!   contest in `<cwd>/<Contest>/`, `receive stop` stops listening.
//!
//! It runs your compiler and your program, so it needs (plugins.toml):
//!
//! ```toml
//! [competitest]
//! allow-process = true
//! allow-files = "write"          # to create testcases
//! allow-listen = [27121]         # for Competitive Companion
//! timeout = 2000                 # ms per testcase
//! # $(FNAME) is the file, $(FNOEXT) without its extension:
//! compile.cpp = ["g++", "-O2", "-std=c++17", "-o", "$(FNOEXT)", "$(FNAME)"]
//! run.cpp = ["./$(FNOEXT)"]
//! companion-port = 27121         # Competitive Companion's default
//! always-listen = false          # receive problems whenever they are sent
//! received-extension = "cpp"     # for `receive problem` / `receive contest`
//! ```
//!
//! New problem files start from `template.<ext>` in the plugin's data
//! directory (`~/.local/share/helix/plugins/competitest/`), if it exists,
//! with `$(PROBLEM)`, `$(GROUP)`, `$(URL)`, `$(TIMELIM)` and `$(MEMLIM)`
//! filled in.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use helix_plugin_api::{
    editor, listener,
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
    receive: Receive,
}

/// Receiving problems from Competitive Companion.
#[derive(Default)]
struct Receive {
    port: u16,
    /// Keep listening after a problem arrives (`always-listen`).
    always: bool,
    extension: String,
    /// The running listener.
    listener: Option<u32>,
    /// What to do with the next problem; `None` while listening only
    /// because of `always-listen`: then make problem files.
    target: Option<Target>,
    /// The contest batch being received: (id, problems so far).
    batch: Option<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Target {
    /// Replace the testcases of this file.
    Testcases(PathBuf),
    Problem,
    Contest,
}

/// What Competitive Companion sends for one problem (the fields used).
#[derive(Debug, Clone, PartialEq)]
struct Problem {
    name: String,
    group: String,
    url: String,
    /// ms
    time_limit: u64,
    /// MB
    memory_limit: u64,
    tests: Vec<(String, String)>,
    /// (id, number of problems): a contest arrives one problem at a time.
    batch: (String, usize),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn parse_problem(body: &str) -> Result<Problem, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("not a Competitive Companion problem: {err}"))?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let name = text("name");
    if name.trim().is_empty() {
        return Err("not a Competitive Companion problem: no name".into());
    }
    let tests = value
        .get("tests")
        .and_then(|t| t.as_array())
        .ok_or("not a Competitive Companion problem: no tests")?
        .iter()
        .map(|test| {
            let field = |key| test.get(key).and_then(|v: &serde_json::Value| v.as_str()).map(str::to_string);
            Some((field("input")?, field("output")?))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("not a Competitive Companion problem: a test has no input or output")?;
    let batch = value.get("batch");
    Ok(Problem {
        name,
        group: text("group"),
        url: text("url"),
        time_limit: value.get("timeLimit").and_then(|v| v.as_u64()).unwrap_or_default(),
        memory_limit: value.get("memoryLimit").and_then(|v| v.as_u64()).unwrap_or_default(),
        tests,
        batch: (
            batch.and_then(|b| b.get("id")).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            batch.and_then(|b| b.get("size")).and_then(|v| v.as_u64()).map_or(1, |n| n.max(1) as usize),
        ),
    })
}

/// `name` as a file name: letters, digits, `-` and `_`; other runs become
/// one `_` ("A. Watermelon" is "A_Watermelon").
fn file_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_');
    if out.is_empty() {
        "problem".into()
    } else {
        out.chars().take(100).collect()
    }
}

fn fill_template(template: &str, problem: &Problem) -> String {
    template
        .replace("$(PROBLEM)", &problem.name)
        .replace("$(GROUP)", &problem.group)
        .replace("$(URL)", &problem.url)
        .replace("$(TIMELIM)", &problem.time_limit.to_string())
        .replace("$(MEMLIM)", &problem.memory_limit.to_string())
}

/// The testcase files of `file` that exist.
fn testcase_files(file: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (file.parent(), file.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            ["input", "output"].iter().any(|kind| {
                name.strip_prefix(&format!("{stem}_{kind}"))
                    .and_then(|rest| rest.strip_suffix(".txt"))
                    .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            })
        })
        .collect()
}

/// Replace `file`'s testcases with `tests`.
fn write_testcases(file: &Path, tests: &[(String, String)]) -> Result<(), String> {
    let dir = file.parent().ok_or("no directory")?;
    let stem = file.file_stem().ok_or("no file name")?.to_string_lossy().into_owned();
    let error = |path: &Path, err: std::io::Error| {
        format!("{}: {err} (allow-files = \"write\" in plugins.toml)", path.display())
    };
    for old in testcase_files(file) {
        std::fs::remove_file(&old).map_err(|err| error(&old, err))?;
    }
    for (number, (input, output)) in tests.iter().enumerate() {
        let input_path = dir.join(format!("{stem}_input{number}.txt"));
        let output_path = dir.join(format!("{stem}_output{number}.txt"));
        std::fs::write(&input_path, input).map_err(|err| error(&input_path, err))?;
        std::fs::write(&output_path, output).map_err(|err| error(&output_path, err))?;
    }
    Ok(())
}

/// Make `dir/<Problem>.<ext>` (unless it exists) and its testcases; the
/// file.
fn write_problem(dir: &Path, problem: &Problem, extension: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let file = dir.join(format!("{}.{extension}", file_name(&problem.name)));
    if !file.exists() {
        let template = Path::new(&editor::data_dir()).join(format!("template.{extension}"));
        let text = std::fs::read_to_string(template)
            .map(|template| fill_template(&template, problem))
            .unwrap_or_default();
        std::fs::write(&file, text).map_err(|err| {
            format!("{}: {err} (allow-files = \"write\" in plugins.toml)", file.display())
        })?;
    }
    write_testcases(&file, &problem.tests)?;
    Ok(file)
}

/// Start listening for Competitive Companion, if not already.
fn start_listening() -> Result<(), String> {
    let port = STATE.with_borrow(|state| {
        (state.receive.listener.is_none()).then_some(state.receive.port)
    });
    let Some(port) = port else {
        return Ok(());
    };
    let id = listener::listen(port, |body| received(body), move |error| {
        STATE.with_borrow_mut(|state| {
            state.receive.listener = None;
            state.receive.target = None;
        });
        editor::set_error(&format!("CompetiTest: {error}"));
        Ok(())
    })?;
    STATE.with_borrow_mut(|state| state.receive.listener = Some(id));
    Ok(())
}

fn stop_listening() {
    let listener = STATE.with_borrow_mut(|state| {
        state.receive.target = None;
        state.receive.batch = None;
        state.receive.listener.take()
    });
    if let Some(id) = listener {
        editor::cancel(id);
    }
}

/// `receive [testcases|problem|contest|stop]`.
fn receive(what: &str) -> Result<(), String> {
    let target = match what {
        "" | "testcases" => Target::Testcases(current_file()?),
        "problem" => Target::Problem,
        "contest" => Target::Contest,
        "stop" => {
            stop_listening();
            editor::set_status("CompetiTest: stopped receiving");
            return Ok(());
        }
        _ => return Err(format!("receive what? testcases, problem, contest or stop, not '{what}'")),
    };
    start_listening()?;
    let port = STATE.with_borrow_mut(|state| {
        state.receive.target = Some(target.clone());
        state.receive.batch = None;
        state.receive.port
    });
    let what = match target {
        Target::Testcases(_) => "testcases",
        Target::Problem => "a problem",
        Target::Contest => "a contest",
    };
    editor::set_status(&format!(
        "CompetiTest: waiting for {what} from Competitive Companion (port {port}); `receive stop` stops"
    ));
    Ok(())
}

/// A request from Competitive Companion.
fn received(body: &str) -> Result<(), String> {
    let problem = match parse_problem(body) {
        Ok(problem) => problem,
        Err(err) => {
            editor::log(&err);
            return Ok(());
        }
    };
    let (target, extension) = STATE.with_borrow(|state| {
        (state.receive.target.clone(), state.receive.extension.clone())
    });
    let tests = problem.tests.len();
    // A batch of several problems is a contest even when listening only
    // because of `always-listen`.
    let contest = matches!(target, Some(Target::Contest)) || (target.is_none() && problem.batch.1 > 1);
    let done = match target {
        Some(Target::Testcases(file)) => {
            write_testcases(&file, &problem.tests)?;
            let name = file.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            editor::set_status(&format!("CompetiTest: {tests} testcases of \"{}\" for {name}", problem.name));
            if STATE.with_borrow(|state| state.file.as_ref() == Some(&file)) {
                STATE.with_borrow_mut(|state| {
                    state.testcases = find_testcases(&file);
                    state.compile_error = None;
                });
                show_panel()?;
            }
            true
        }
        _ if contest => {
            let dir = Path::new(&editor::cwd()).join(file_name(&problem.group));
            write_problem(&dir, &problem, &extension)?;
            let (got, size) = STATE.with_borrow_mut(|state| {
                let batch = &mut state.receive.batch;
                match batch {
                    Some((id, got)) if *id == problem.batch.0 => *got += 1,
                    _ => *batch = Some((problem.batch.0.clone(), 1)),
                }
                (batch.as_ref().map_or(1, |(_, got)| *got), problem.batch.1)
            });
            editor::set_status(&format!(
                "CompetiTest: {got}/{size} problems of \"{}\" in {}",
                problem.group,
                dir.display()
            ));
            got >= size
        }
        _ => {
            let file = write_problem(Path::new(&editor::cwd()), &problem, &extension)?;
            editor::open_file(&file.to_string_lossy())?;
            editor::set_status(&format!("CompetiTest: \"{}\" with {tests} testcases", problem.name));
            true
        }
    };
    if done {
        let keep = STATE.with_borrow_mut(|state| {
            state.receive.target = None;
            state.receive.batch = None;
            state.receive.always
        });
        if !keep {
            stop_listening();
        }
    }
    Ok(())
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

/// Settings: `companion-port`, `always-listen`, `received-extension`.
fn parse_receive_settings(config: &str) -> Result<Receive, String> {
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|err| format!("invalid settings: {err}"))?;
    let port = match value.get("companion-port") {
        None => 27121,
        Some(port) => port
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0)
            .ok_or("companion-port must be a port number")?,
    };
    let extension = match value.get("received-extension") {
        None => "cpp".to_string(),
        Some(ext) => ext
            .as_str()
            .map(|ext| ext.trim_start_matches('.').to_string())
            .filter(|ext| !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .ok_or("received-extension must be an extension like \"cpp\"")?,
    };
    Ok(Receive {
        port,
        always: value.get("always-listen").and_then(|v| v.as_bool()).unwrap_or(false),
        extension,
        ..Default::default()
    })
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
        let receive = parse_receive_settings(&config)?;
        let always = receive.always;
        STATE.with_borrow_mut(|state| {
            state.timeout = timeout;
            state.commands = commands;
            state.receive = receive;
        });
        if always {
            start_listening()?;
        }
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
                command(
                    "receive",
                    "Receive from Competitive Companion: testcases (default), problem, contest or stop",
                ),
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
            "receive" => receive(args.first().map_or("", String::as_str)),
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

    const COMPANION: &str = r#"{
        "name": "A. Watermelon",
        "group": "Codeforces - Beta Round #4 (Div. 2 Only)",
        "url": "https://codeforces.com/problemset/problem/4/A",
        "interactive": false,
        "memoryLimit": 64,
        "timeLimit": 1000,
        "tests": [{"input": "8\n", "output": "YES\n"}, {"input": "5\n", "output": "NO\n"}],
        "testType": "single",
        "input": {"type": "stdin"},
        "output": {"type": "stdout"},
        "languages": {"java": {"mainClass": "Main", "taskClass": "AWatermelon"}},
        "batch": {"id": "123e4567-e89b-12d3-a456-426614174000", "size": 1}
    }"#;

    #[test]
    fn parses_competitive_companion_problems() {
        let problem = parse_problem(COMPANION).unwrap();
        assert_eq!(problem.name, "A. Watermelon");
        assert_eq!(problem.tests, [("8\n".into(), "YES\n".into()), ("5\n".into(), "NO\n".into())]);
        assert_eq!((problem.time_limit, problem.memory_limit), (1000, 64));
        assert_eq!(problem.batch.1, 1);
        assert!(parse_problem("{}").is_err());
        assert!(parse_problem(r#"{"name": "x", "tests": [{"input": "1"}]}"#).is_err());
        assert!(parse_problem("nope").is_err());
    }

    #[test]
    fn names_and_templates() {
        assert_eq!(file_name("A. Watermelon"), "A_Watermelon");
        assert_eq!(file_name("Codeforces - Beta Round #4 (Div. 2 Only)"), "Codeforces_-_Beta_Round_4_Div_2_Only");
        assert_eq!(file_name("../../etc"), "etc");
        assert_eq!(file_name("/// "), "problem");
        let problem = parse_problem(COMPANION).unwrap();
        assert_eq!(fill_template("// $(PROBLEM) $(TIMELIM)ms $(MEMLIM)MB $(URL)", &problem), "// A. Watermelon 1000ms 64MB https://codeforces.com/problemset/problem/4/A");
    }

    #[test]
    fn receive_settings() {
        let receive = parse_receive_settings("{}").unwrap();
        assert_eq!((receive.port, receive.always, receive.extension.as_str()), (27121, false, "cpp"));
        let receive = parse_receive_settings(r#"{"companion-port": 10045, "always-listen": true, "received-extension": ".py"}"#).unwrap();
        assert_eq!((receive.port, receive.always, receive.extension.as_str()), (10045, true, "py"));
        assert!(parse_receive_settings(r#"{"companion-port": 70000}"#).is_err());
        assert!(parse_receive_settings(r#"{"received-extension": "../x"}"#).is_err());
    }

    #[test]
    fn received_testcases_replace_the_old_ones() {
        let dir = std::env::temp_dir().join(format!("competitest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sol.cpp");
        for n in 0..3 {
            std::fs::write(dir.join(format!("sol_input{n}.txt")), "old").unwrap();
            std::fs::write(dir.join(format!("sol_output{n}.txt")), "old").unwrap();
        }
        std::fs::write(dir.join("sol_inputs.txt"), "not a testcase").unwrap();
        write_testcases(&file, &parse_problem(COMPANION).unwrap().tests).unwrap();
        let found = find_testcases(&file);
        assert_eq!(found.len(), 2);
        assert_eq!(std::fs::read_to_string(&found[1].expected).unwrap(), "NO\n");
        assert!(!dir.join("sol_input2.txt").exists());
        assert!(dir.join("sol_inputs.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
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
