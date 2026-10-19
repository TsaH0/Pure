//! Programs started by plugins: `spawn` (in the background, with captured
//! output) and `run-interactive` (with the terminal handed over).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use helix_event::runtime_local;
use helix_plugin::{Callback, ProcessOutput};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::interactive::Interactive;

/// Most output kept per stream; the rest is read and dropped.
const MAX_OUTPUT: usize = 16 << 20;
/// After a program exits, how long to wait for the rest of its output. A
/// child it started may keep the pipes open.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);

runtime_local! {
    /// Running programs by `(plugin, request id)`. Dropping the sender kills
    /// the program.
    static RUNNING: Mutex<BTreeMap<(String, u32), oneshot::Sender<()>>> = Mutex::new(BTreeMap::new());
}

/// Run `argv` in the background and deliver its output to the plugin.
pub fn spawn(
    plugin: String,
    id: u32,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    timeout: Duration,
) {
    let (kill_tx, kill_rx) = oneshot::channel();
    RUNNING.lock().insert((plugin.clone(), id), kill_tx);
    tokio::spawn(async move {
        let result = run(argv, cwd, stdin, timeout, kill_rx).await;
        RUNNING.lock().remove(&(plugin.clone(), id));
        super::deliver_later(plugin, Callback::ProcessDone { id, result });
    });
}

/// Kill a program started with [`spawn`]. Its result is still delivered.
pub fn kill(plugin: &str, id: u32) {
    // Dropping the sender is enough.
    RUNNING.lock().remove(&(plugin.to_string(), id));
}

/// Kill every plugin's programs (when plugins are reloaded).
pub fn kill_all() {
    RUNNING.lock().clear();
}

async fn run(
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    timeout: Duration,
    kill: oneshot::Receiver<()>,
) -> Result<ProcessOutput, String> {
    let program = &argv[0];
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .current_dir(cwd.unwrap_or_else(helix_stdx::env::current_working_dir))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to run '{program}': {err}"))?;

    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Closed when written (or when the program stops reading).
        tokio::spawn(async move {
            let _ = pipe.write_all(input.as_bytes()).await;
        });
    }
    let stdout = Capture::start(child.stdout.take());
    let stderr = Capture::start(child.stderr.take());

    let mut note = None;
    let status = tokio::select! {
        status = child.wait() => status.ok(),
        _ = tokio::time::sleep(timeout) => {
            note = Some(format!("[killed after {} ms]", timeout.as_millis()));
            let _ = child.kill().await;
            None
        }
        _ = kill => {
            note = Some("[killed]".to_string());
            let _ = child.kill().await;
            None
        }
    };

    let stdout = stdout.finish().await;
    let mut stderr = stderr.finish().await;
    for (truncated, name) in [(stdout.1, "stdout"), (stderr.1, "stderr")] {
        if truncated {
            stderr.0.push_str(&format!("\n[{name} truncated at 16 MiB]"));
        }
    }
    if let Some(note) = note {
        if !stderr.0.is_empty() && !stderr.0.ends_with('\n') {
            stderr.0.push('\n');
        }
        stderr.0.push_str(&note);
    }
    Ok(ProcessOutput {
        exit_code: status.and_then(|status| status.code()),
        stdout: stdout.0,
        stderr: stderr.0,
    })
}

/// Output of one stream, read in the background.
struct Capture {
    buffer: Arc<Mutex<Vec<u8>>>,
    truncated: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Capture {
    fn start(reader: Option<impl AsyncRead + Unpin + Send + 'static>) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let truncated = Arc::new(AtomicBool::new(false));
        let task = reader.map(|mut reader| {
            let (buffer, truncated) = (buffer.clone(), truncated.clone());
            tokio::spawn(async move {
                let mut chunk = [0; 8192];
                while let Ok(n @ 1..) = reader.read(&mut chunk).await {
                    let mut buffer = buffer.lock();
                    let room = MAX_OUTPUT - buffer.len();
                    if n > room {
                        truncated.store(true, Ordering::Relaxed);
                    }
                    buffer.extend_from_slice(&chunk[..n.min(room)]);
                }
            })
        });
        Self {
            buffer,
            truncated,
            task,
        }
    }

    /// What was read, waiting a little for the rest. Lossy UTF-8.
    async fn finish(mut self) -> (String, bool) {
        if let Some(task) = self.task.take() {
            let abort = task.abort_handle();
            if tokio::time::timeout(OUTPUT_GRACE, task).await.is_err() {
                abort.abort();
            }
        }
        let bytes = std::mem::take(&mut *self.buffer.lock());
        (
            String::from_utf8_lossy(&bytes).into_owned(),
            self.truncated.load(Ordering::Relaxed),
        )
    }
}

/// Hand the terminal to `argv`, then deliver its exit code and, with
/// `chooser_file`, what it wrote to the file substituted for `{chooser}`.
pub fn interactive(
    plugin: String,
    id: u32,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    chooser_file: bool,
    wait_for_key: bool,
) -> Result<Interactive, String> {
    let chooser = if chooser_file {
        Some(create_chooser_file().map_err(|err| format!("failed to create a chooser file: {err}"))?)
    } else {
        None
    };
    let args = argv.iter().map(|arg| match &chooser {
        Some(path) => arg.replace("{chooser}", &path.to_string_lossy()),
        None => arg.clone(),
    });
    let args: Vec<String> = args.collect();
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .current_dir(cwd.unwrap_or_else(helix_stdx::env::current_working_dir));
    let mut interactive = Interactive::new(command, move |_editor, _, status| {
        let chosen = chooser.as_ref().map(|path| {
            let chosen = std::fs::read(path).unwrap_or_default();
            let _ = std::fs::remove_file(path);
            String::from_utf8_lossy(&chosen).into_owned()
        });
        let result = status
            .map(|status| ProcessOutput {
                exit_code: status.code(),
                stdout: chosen.unwrap_or_default(),
                stderr: String::new(),
            })
            .map_err(|err| format!("failed to run '{}': {err}", args[0]));
        super::deliver_later(plugin, Callback::ProcessDone { id, result });
    });
    if wait_for_key {
        // Like `space z`: the output stays on a cleared screen until a key.
        interactive.clear_screen = true;
        interactive.wait_for_key = true;
    }
    Ok(interactive)
}

/// A new, empty file only the user can read, for a program to write its
/// choice to.
fn create_chooser_file() -> std::io::Result<PathBuf> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    loop {
        let path = std::env::temp_dir().join(format!(
            "pure-chooser-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match options.open(&path) {
            Ok(_) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    async fn run_sh(script: &str, stdin: Option<&str>, timeout: Duration) -> ProcessOutput {
        let (_kill, kill_rx) = oneshot::channel();
        run(sh(script), None, stdin.map(str::to_owned), timeout, kill_rx)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn output_and_exit_code() {
        let out = run_sh("echo hi; echo err >&2; exit 3", None, Duration::from_secs(5)).await;
        assert_eq!(out.exit_code, Some(3));
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");
    }

    #[tokio::test]
    async fn stdin_cwd_and_lossy_utf8() {
        let out = run_sh("cat; printf '\\377'", Some("in put"), Duration::from_secs(5)).await;
        assert_eq!(out.stdout, "in put\u{FFFD}");
        let dir = tempfile::tempdir().unwrap();
        let (_kill, kill_rx) = oneshot::channel();
        let out = run(
            sh("pwd"),
            Some(dir.path().to_path_buf()),
            None,
            Duration::from_secs(5),
            kill_rx,
        )
        .await
        .unwrap();
        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(
            PathBuf::from(out.stdout.trim()).canonicalize().unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn timeout_and_kill() {
        let started = std::time::Instant::now();
        // The background child keeps the pipes open after `sh` is killed.
        let out = run_sh("sleep 30 & sleep 30", None, Duration::from_millis(200)).await;
        assert_eq!(out.exit_code, None);
        assert!(out.stderr.contains("killed after 200 ms"), "{}", out.stderr);
        assert!(started.elapsed() < Duration::from_secs(5));

        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(run(sh("sleep 30"), None, None, Duration::from_secs(30), kill_rx));
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(kill_tx);
        let out = task.await.unwrap().unwrap();
        assert_eq!(out.exit_code, None);
        assert!(out.stderr.ends_with("[killed]"), "{}", out.stderr);
    }

    #[tokio::test]
    async fn missing_program() {
        let (_kill, kill_rx) = oneshot::channel();
        let err = run(
            vec!["/nonexistent/program".into()],
            None,
            None,
            Duration::from_secs(1),
            kill_rx,
        )
        .await
        .unwrap_err();
        assert!(err.contains("failed to run '/nonexistent/program'"), "{err}");
    }

    #[test]
    fn chooser_files_are_new_and_private() {
        let a = create_chooser_file().unwrap();
        let b = create_chooser_file().unwrap();
        assert_ne!(a, b);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
    }
}
