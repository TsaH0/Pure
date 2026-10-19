//! Closure routing for API 0.2 requests.
use std::cell::RefCell;
use std::collections::HashMap;

use crate::bindings::helix::plugin::{process, ui};
use crate::{Item, OpenAction, Output, Picker, PickerResult, RequestId};

type ProcessCallback = Box<dyn FnOnce(Result<Output, String>) -> Result<(), String>>;
type PickerCallback = Box<dyn FnOnce(PickerResult) -> Result<(), String>>;

enum Completion {
    Process(ProcessCallback),
    Picker(PickerCallback),
}

thread_local! {
    static PENDING: RefCell<HashMap<RequestId, Completion>> = RefCell::new(HashMap::new());
}

/// A picker result. Cancellation has no items, indices or open action.
#[derive(Debug, Clone)]
pub struct Choice {
    /// Chosen items, in the order returned by the picker.
    pub items: Vec<Item>,
    /// Indices in the original item list.
    pub indices: Vec<u32>,
    /// Enter replaces the view; C-s and C-v request splits. `None` on cancel.
    pub action: Option<OpenAction>,
}

/// Background process options. Unset fields use the editor's working
/// directory, closed stdin, and a 30 second timeout.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Working directory, defaulting to the editor's.
    pub cwd: Option<String>,
    /// Text written to stdin before closing it.
    pub stdin: Option<String>,
    /// Timeout in milliseconds, at most ten minutes.
    pub timeout_ms: Option<u32>,
}

impl From<SpawnOptions> for process::SpawnOptions {
    fn from(options: SpawnOptions) -> Self {
        Self {
            cwd: options.cwd,
            stdin: options.stdin,
            timeout_ms: options.timeout_ms,
        }
    }
}

/// Terminal hand-over options. All options are disabled by default.
#[derive(Debug, Clone, Default)]
pub struct InteractiveOptions {
    /// Working directory, defaulting to the editor's.
    pub cwd: Option<String>,
    /// Substitute a temporary chooser file and return its contents.
    pub chooser_file: bool,
    /// Wait for a key on the normal screen after the program exits.
    pub wait_for_key: bool,
}

impl From<InteractiveOptions> for process::InteractiveOptions {
    fn from(options: InteractiveOptions) -> Self {
        Self {
            cwd: options.cwd,
            chooser_file: options.chooser_file,
            wait_for_key: options.wait_for_key,
        }
    }
}

/// Kill a background request. Its completion callback is still delivered.
pub fn kill(id: RequestId) {
    process::kill(id)
}

impl Item {
    /// A one-column item whose display and data are both `text`.
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            columns: vec![text.clone()],
            data: text,
            preview: None,
        }
    }
}

impl Picker {
    /// A single-column picker, initially selecting its first item.
    pub fn new(title: impl Into<String>, items: Vec<Item>) -> Self {
        Self {
            title: title.into(),
            columns: vec!["Name".into()],
            items,
            initial: None,
        }
    }
}

/// Start a background program without a shell. Its closure runs once with
/// captured output or an error, and may start another request. The returned id
/// can be passed to [`crate::editor::kill`]. Requires a process permission and grant.
pub fn spawn(
    argv: &[impl AsRef<str>],
    options: SpawnOptions,
    callback: impl FnOnce(Result<Output, String>) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let argv: Vec<_> = argv.iter().map(|arg| arg.as_ref().to_owned()).collect();
    let id = process::spawn(&argv, &options.into())?;
    register_process(id, callback);
    Ok(id)
}

/// Hand the terminal to a program and receive its exit status and optional
/// chooser file in a one-shot callback. Only from commands and callbacks;
/// requires a process permission and grant.
pub fn run_interactive(
    argv: &[impl AsRef<str>],
    options: InteractiveOptions,
    callback: impl FnOnce(Result<Output, String>) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let argv: Vec<_> = argv.iter().map(|arg| arg.as_ref().to_owned()).collect();
    let id = process::run_interactive(&argv, &options.into())?;
    register_process(id, callback);
    Ok(id)
}

/// Show a picker and call `callback` once on selection or cancellation. Item
/// data and split actions are preserved. Only from commands and callbacks.
pub fn pick(
    picker: Picker,
    callback: impl FnOnce(Choice) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let items = picker.items.clone();
    let id = ui::show_picker(&picker)?;
    register_picker(id, move |result| {
        let choice = match result {
            PickerResult::Cancelled => Choice {
                items: Vec::new(),
                indices: Vec::new(),
                action: None,
            },
            PickerResult::Accepted((indices, action)) => {
                let chosen = indices
                    .iter()
                    .map(|&index| {
                        items
                            .get(index as usize)
                            .cloned()
                            .ok_or_else(|| format!("invalid picker index {index}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Choice {
                    items: chosen,
                    indices,
                    action: Some(action),
                }
            }
        };
        callback(choice)
    });
    Ok(id)
}

#[doc(hidden)]
pub fn register_process(
    id: RequestId,
    callback: impl FnOnce(Result<Output, String>) -> Result<(), String> + 'static,
) {
    PENDING.with_borrow_mut(|pending| {
        pending.insert(id, Completion::Process(Box::new(callback)));
    });
}

#[doc(hidden)]
pub fn register_picker(
    id: RequestId,
    callback: impl FnOnce(PickerResult) -> Result<(), String> + 'static,
) {
    PENDING.with_borrow_mut(|pending| {
        pending.insert(id, Completion::Picker(Box::new(callback)));
    });
}

#[doc(hidden)]
pub fn process_done(id: RequestId, result: Result<Output, String>) {
    let callback = PENDING.with_borrow_mut(|pending| {
        if matches!(pending.get(&id), Some(Completion::Process(_))) {
            pending.remove(&id)
        } else {
            None
        }
    });
    if let Some(Completion::Process(callback)) = callback {
        report(callback(result));
    }
}

#[doc(hidden)]
pub fn picker_done(id: RequestId, result: PickerResult) {
    let callback = PENDING.with_borrow_mut(|pending| {
        if matches!(pending.get(&id), Some(Completion::Picker(_))) {
            pending.remove(&id)
        } else {
            None
        }
    });
    if let Some(Completion::Picker(callback)) = callback {
        report(callback(result));
    }
}

pub(crate) fn run_command(f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let existing: std::collections::HashSet<_> =
        PENDING.with_borrow(|pending| pending.keys().copied().collect());
    let result = f();
    if result.is_err() {
        PENDING.with_borrow_mut(|pending| pending.retain(|id, _| existing.contains(id)));
    }
    result
}

fn report(result: Result<(), String>) {
    if let Err(err) = result {
        crate::editor::set_error(&format!("plugin callback: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn failed_commands_drop_only_their_new_callbacks() {
        register_process(10, |_| Ok(()));
        let result = run_command(|| {
            register_process(11, |_| Ok(()));
            Err("failed".into())
        });
        assert!(result.is_err());
        PENDING.with_borrow(|pending| {
            assert!(pending.contains_key(&10));
            assert!(!pending.contains_key(&11));
        });
        process_done(10, Err("cleanup".into()));
    }

    #[test]
    fn callbacks_are_one_shot_and_can_chain_without_borrowing_the_registry() {
        let received = Rc::new(Cell::new(0));
        let first = received.clone();
        register_process(1, move |result| {
            assert_eq!(result.unwrap().stdout, "hello");
            first.set(1);
            register_picker(2, move |result| {
                assert!(matches!(result, PickerResult::Cancelled));
                first.set(2);
                Ok(())
            });
            Ok(())
        });
        process_done(999, Err("unknown".into()));
        picker_done(1, PickerResult::Cancelled);
        assert_eq!(received.get(), 0);
        process_done(
            1,
            Ok(Output {
                exit_code: Some(0),
                stdout: "hello".into(),
                stderr: String::new(),
            }),
        );
        process_done(1, Err("duplicate".into()));
        assert_eq!(received.get(), 1);
        picker_done(2, PickerResult::Cancelled);
        picker_done(2, PickerResult::Cancelled);
        assert_eq!(received.get(), 2);
    }
}
