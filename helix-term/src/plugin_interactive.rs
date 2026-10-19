//! Deferred terminal hand-over and chooser results for granted plugin requests.
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, LazyLock};

use helix_event::runtime_local;
use helix_plugin::{Callback, InteractiveOptions, PendingKind, ProcessOutput};
use parking_lot::Mutex;
use tokio::process::Command;

use crate::interactive::Interactive;
use crate::job::Jobs;
use crate::plugin_processes::OUTPUT_LIMIT;

type Active = Arc<Mutex<Option<(String, u32)>>>;
runtime_local! {
    static ACTIVE: LazyLock<Active> = LazyLock::new(|| Arc::new(Mutex::new(None)));
}

struct Request {
    active: Active,
    plugin: String,
    id: u32,
    completed: bool,
}

impl Request {
    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        let mut active = self.active.lock();
        if active
            .as_ref()
            .is_some_and(|(plugin, id)| plugin == &self.plugin && *id == self.id)
        {
            *active = None;
        }
        drop(active);
        if !self.completed {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(deliver(
                    self.plugin.clone(),
                    self.id,
                    Err("interactive request cancelled before starting".into()),
                ));
            }
        }
    }
}

fn pending(plugin: &str, id: u32) -> bool {
    crate::plugins::with_host(|host| host.request_is_pending(plugin, id, PendingKind::Process))
        .unwrap_or(false)
}

pub(crate) fn request(
    plugin: String,
    id: u32,
    argv: Vec<String>,
    options: InteractiveOptions,
    jobs: &mut Jobs,
) {
    if !pending(&plugin, id) {
        return;
    }
    let mut active = ACTIVE.lock();
    if active.is_some() {
        tokio::spawn(deliver(
            plugin,
            id,
            Err("another interactive program is already pending".into()),
        ));
        return;
    }
    *active = Some((plugin.clone(), id));
    drop(active);
    let request = Request {
        active: Arc::clone(&ACTIVE),
        plugin: plugin.clone(),
        id,
        completed: false,
    };
    let cwd = options
        .cwd
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(helix_stdx::env::current_working_dir);
    let prepared = if options.chooser_file {
        crate::interactive::with_chooser(&argv, Some(&cwd))
            .map(|(interactive, chooser)| (interactive, Some(chooser)))
    } else {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).current_dir(&cwd);
        Ok((Interactive::new(command, |_, _, _| {}), None))
    };
    let (mut interactive, chooser) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            request.complete();
            tokio::spawn(deliver(
                plugin,
                id,
                Err(format!("failed to prepare interactive program: {err}")),
            ));
            return;
        }
    };
    interactive.command.kill_on_drop(true);
    interactive.normal_screen = options.wait_for_key;
    interactive.clear_screen = options.wait_for_key;
    interactive.wait_for_key = options.wait_for_key;
    let check_plugin = plugin.clone();
    interactive.before_start = Some(Box::new(move || pending(&check_plugin, id)));
    let ready_plugin = plugin.clone();
    interactive.on_exit = Box::new(move |_, _, status| {
        request.complete();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || finish(status, chooser))
                .await
                .unwrap_or_else(|err| Err(format!("failed reading interactive result: {err}")));
            deliver(plugin, id, result).await;
        });
    });
    crate::interactive::request_if(jobs, interactive, move |_, compositor| {
        compositor.interactive.is_none() && pending(&ready_plugin, id)
    });
}

async fn deliver(plugin: String, id: u32, result: Result<ProcessOutput, String>) {
    crate::job::dispatch_with_jobs(move |editor, _, jobs| {
        crate::plugins::deliver_callback(
            editor,
            jobs,
            &plugin,
            Callback::ProcessDone { id, result },
        );
    })
    .await;
}

pub(crate) fn cancel_all() {
    ACTIVE.lock().take();
}

pub(crate) fn cancel_plugin(plugin: &str) {
    let mut active = ACTIVE.lock();
    if active.as_ref().is_some_and(|(name, _)| name == plugin) {
        active.take();
    }
}

fn finish(
    status: io::Result<ExitStatus>,
    chooser: Option<tempfile::TempPath>,
) -> Result<ProcessOutput, String> {
    let status = status.map_err(|err| format!("failed to run interactive program: {err}"))?;
    let (stdout, truncated) = chooser
        .as_ref()
        .map(|path| read_chooser(path))
        .transpose()?
        .unwrap_or_default();
    drop(chooser);
    Ok(ProcessOutput {
        exit_code: status.code(),
        stdout,
        stderr: if truncated {
            "[chooser output truncated at 16 MiB]\n".into()
        } else {
            String::new()
        },
    })
}

fn read_chooser(path: &Path) -> Result<(String, bool), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok((String::new(), false)),
        Err(err) => return Err(format!("failed reading chooser: {err}")),
    };
    if !file.metadata().map_err(|err| err.to_string())?.is_file() {
        return Err("chooser is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    let truncated = bytes.len() > OUTPUT_LIMIT || text.len() > OUTPUT_LIMIT;
    let mut end = text.len().min(OUTPUT_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    Ok((text, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn success() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }

    #[cfg(windows)]
    fn success() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }

    #[test]
    fn chooser_results_are_lossy_bounded_and_removed_after_exit() {
        let chooser = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let path = chooser.to_path_buf();
        std::fs::write(&path, b"picked-\xff\n").unwrap();
        let result = finish(Ok(success()), Some(chooser)).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "picked-\u{fffd}\n");
        assert!(!path.exists());
        let chooser = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let path = chooser.to_path_buf();
        std::fs::write(&path, vec![b'x'; OUTPUT_LIMIT + 1]).unwrap();
        let result = finish(Ok(success()), Some(chooser)).unwrap();
        assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
        assert!(result.stderr.contains("truncated"));
        assert!(!path.exists());
    }

    #[test]
    fn failed_programs_and_unlaunched_choosers_clean_up() {
        let chooser = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let path = chooser.to_path_buf();
        assert!(finish(Err(io::Error::other("failed")), Some(chooser)).is_err());
        assert!(!path.exists());
        let chooser = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let path = chooser.to_path_buf();
        let on_exit: crate::interactive::OnExit = Box::new(move |_, _, status| {
            let _ = finish(status, Some(chooser));
        });
        drop(on_exit);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn chooser_cannot_block_on_a_fifo() {
        use std::ffi::CString;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipe");
        let name = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: create a FIFO at an owned, absent temporary path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            read_chooser(&path).unwrap_err(),
            "chooser is not a regular file"
        );
    }
}
