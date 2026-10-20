//! Closure routing for API 0.2 and 0.3 requests.
use std::cell::RefCell;
use std::collections::HashMap;

use crate::bindings::helix::plugin::{process, ui};
use crate::{Item, OpenAction, Output, Picker, PickerResult, RequestId};

type ProcessCallback = Box<dyn FnOnce(Result<Output, String>) -> Result<(), String>>;
type PickerCallback = Box<dyn FnOnce(PickerResult) -> Result<(), String>>;
type PromptCallback = Box<dyn FnOnce(Option<String>) -> Result<(), String>>;
#[cfg(feature = "api-v0-2")]
type QueryCallback = Box<dyn FnMut(RequestId, &str) -> Result<Option<Vec<Item>>, String>>;
#[cfg(not(feature = "api-v0-2"))]
type QueryCallback = Box<dyn FnMut(RequestId, u64, &str) -> Result<Option<Vec<Item>>, String>>;
#[cfg(not(feature = "api-v0-2"))]
type StreamCallback = Box<dyn FnMut(&str) -> Result<(), String>>;
#[cfg(not(feature = "api-v0-2"))]
type PopupCallback = Box<dyn FnOnce(RequestId) -> Result<(), String>>;
#[cfg(not(feature = "api-v0-2"))]
type BackgroundCallback = Box<dyn FnOnce(Result<String, String>) -> Result<(), String>>;

enum Completion {
    Process(ProcessCallback),
    Picker(PickerCallback),
    Prompt(PromptCallback),
    #[cfg(not(feature = "api-v0-2"))]
    Popup(PopupCallback),
    #[cfg(not(feature = "api-v0-2"))]
    Background(BackgroundCallback),
}

enum ItemUndo {
    Truncate(usize),
    Replace(Vec<Item>),
}

struct CommandTransaction {
    existing: std::collections::HashSet<RequestId>,
    items: HashMap<RequestId, ItemUndo>,
}

thread_local! {
    static PENDING: RefCell<HashMap<RequestId, Completion>> = RefCell::new(HashMap::new());
    /// The items of open pickers, to hand the chosen ones to the callback.
    static PICKER_ITEMS: RefCell<HashMap<RequestId, Vec<Item>>> = RefCell::new(HashMap::new());
    /// Live pickers' query handlers.
    static QUERIES: RefCell<HashMap<RequestId, QueryCallback>> = RefCell::new(HashMap::new());
    static COMMAND: RefCell<Option<CommandTransaction>> = const { RefCell::new(None) };
    #[cfg(not(feature = "api-v0-2"))]
    static STREAMS: RefCell<HashMap<RequestId, StreamCallback>> = RefCell::new(HashMap::new());
    #[cfg(not(feature = "api-v0-2"))]
    static GENERATIONS: RefCell<HashMap<RequestId, u64>> = RefCell::new(HashMap::new());
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
            #[cfg(not(feature = "api-v0-2"))]
            stream: false,
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

    /// An item showing `columns`, handing back `data` when chosen.
    pub fn new(columns: Vec<String>, data: impl Into<String>) -> Self {
        Self {
            columns,
            data: data.into(),
            preview: None,
        }
    }

    /// Show `text` as the item's preview.
    pub fn with_text_preview(mut self, text: impl Into<String>) -> Self {
        self.preview = Some(crate::Preview::Text(text.into()));
        self
    }

    /// Preview the file `path`, highlighting `lines` (0-based, inclusive).
    pub fn with_file_preview(mut self, path: impl Into<String>, lines: Option<(u32, u32)>) -> Self {
        self.preview = Some(crate::Preview::File((path.into(), lines)));
        self
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
            live: false,
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

/// Stream complete stdout lines in bounded batches. The final callback gets
/// only the unfinished stdout tail and the full stderr, after all batches.
#[cfg(not(feature = "api-v0-2"))]
pub fn spawn_stream(
    argv: &[impl AsRef<str>],
    options: SpawnOptions,
    output: impl FnMut(&str) -> Result<(), String> + 'static,
    done: impl FnOnce(Result<Output, String>) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let argv: Vec<_> = argv.iter().map(|arg| arg.as_ref().to_owned()).collect();
    let mut options: process::SpawnOptions = options.into();
    options.stream = true;
    let id = process::spawn(&argv, &options)?;
    register_process(id, done);
    STREAMS.with_borrow_mut(|streams| streams.insert(id, Box::new(output)));
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
    let id = ui::show_picker(&picker)?;
    PICKER_ITEMS.with_borrow_mut(|open| open.insert(id, picker.items));
    register_picker(id, move |result| {
        QUERIES.with_borrow_mut(|queries| queries.remove(&id));
        #[cfg(not(feature = "api-v0-2"))]
        GENERATIONS.with_borrow_mut(|generations| generations.remove(&id));
        let items = PICKER_ITEMS
            .with_borrow_mut(|open| open.remove(&id))
            .unwrap_or_default();
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

/// Show a live picker: instead of filtering `picker`'s items, the editor
/// calls `query` with the picker's id and what the user typed (debounced),
/// and shows the items it returns. To answer later (e.g. from a program
/// started with [`spawn`]), return `None` and call [`set_items`] with the id.
/// `callback` runs once, like for [`pick`].
#[cfg(feature = "api-v0-2")]
pub fn pick_live(
    mut picker: Picker,
    query: impl FnMut(RequestId, &str) -> Result<Option<Vec<Item>>, String> + 'static,
    callback: impl FnOnce(Choice) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    picker.live = true;
    let id = pick(picker, callback)?;
    QUERIES.with_borrow_mut(|queries| queries.insert(id, Box::new(query)));
    Ok(id)
}

/// Show a live picker. Capture the supplied generation for delayed results
/// and use `set_items_for_query` or `push_items_for_query` to return them.
#[cfg(not(feature = "api-v0-2"))]
pub fn pick_live(
    mut picker: Picker,
    query: impl FnMut(RequestId, u64, &str) -> Result<Option<Vec<Item>>, String> + 'static,
    callback: impl FnOnce(Choice) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    picker.live = true;
    let id = pick(picker, callback)?;
    QUERIES.with_borrow_mut(|queries| queries.insert(id, Box::new(query)));
    GENERATIONS.with_borrow_mut(|generations| generations.insert(id, 0));
    Ok(id)
}

/// Replace the items of the open picker `id`.
pub fn set_items(id: RequestId, items: Vec<Item>) -> Result<(), String> {
    set_items_inner(id, None, items)
}

fn set_items_inner(id: RequestId, generation: Option<u64>, items: Vec<Item>) -> Result<(), String> {
    #[cfg(feature = "api-v0-2")]
    {
        let _ = generation;
        ui::set_items(id, &items)?;
    }
    #[cfg(not(feature = "api-v0-2"))]
    ui::set_items(id, generation, &items)?;
    PICKER_ITEMS.with_borrow_mut(|open| {
        if let Some(open) = open.get_mut(&id) {
            let previous = std::mem::replace(open, items);
            record_replacement(id, previous);
        }
    });
    Ok(())
}

/// Replace a live query's results. Obsolete generations and closed pickers
/// are ignored without changing the local item registry.
#[cfg(not(feature = "api-v0-2"))]
pub fn set_items_for_query(id: RequestId, generation: u64, items: Vec<Item>) -> Result<(), String> {
    if accepts_query(id, generation)? {
        set_items_inner(id, Some(generation), items)?;
    }
    Ok(())
}

#[cfg(not(feature = "api-v0-2"))]
fn accepts_query(id: RequestId, generation: u64) -> Result<bool, String> {
    match GENERATIONS.with_borrow(|generations| generations.get(&id).copied()) {
        Some(current) => Ok(current == generation),
        None if PICKER_ITEMS.with_borrow(|open| open.contains_key(&id)) => {
            Err("not a live picker".into())
        }
        None => Ok(false),
    }
}

/// Ask for a line of text. `callback` gets the answer, or `None` if the user
/// pressed Esc. `completions` are offered while typing.
pub fn prompt(
    message: &str,
    initial: &str,
    completions: Vec<String>,
    callback: impl FnOnce(Option<String>) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let id = ui::show_prompt(&ui::Prompt {
        message: message.into(),
        initial: initial.into(),
        completions,
    })?;
    PENDING.with_borrow_mut(|pending| {
        pending.insert(id, Completion::Prompt(Box::new(callback)));
    });
    Ok(id)
}

/// Show markdown in a popup at the cursor, until the next key press.
#[cfg(feature = "api-v0-2")]
pub fn popup(markdown: &str) -> Result<(), String> {
    ui::show_popup(markdown)
}

#[cfg(not(feature = "api-v0-2"))]
#[derive(Debug, Clone, Default)]
pub struct PopupOptions {
    /// Center in the viewport instead of positioning at the cursor.
    pub centered: bool,
}

/// Show a popup and return its owned id, which may be passed to `close_popup`.
#[cfg(not(feature = "api-v0-2"))]
pub fn popup(markdown: &str) -> Result<RequestId, String> {
    popup_with_options(markdown, PopupOptions::default(), |_| Ok(()))
}

/// Show markdown and receive one callback when it closes by id, Esc or a key.
#[cfg(not(feature = "api-v0-2"))]
pub fn popup_with_options(
    markdown: &str,
    options: PopupOptions,
    callback: impl FnOnce(RequestId) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let id = ui::show_popup(&ui::Popup {
        markdown: markdown.into(),
        centered: options.centered,
    })?;
    PENDING.with_borrow_mut(|pending| pending.insert(id, Completion::Popup(Box::new(callback))));
    Ok(id)
}

#[cfg(not(feature = "api-v0-2"))]
pub fn close_popup(id: RequestId) {
    ui::close_popup(id);
}

#[cfg(not(feature = "api-v0-2"))]
#[doc(hidden)]
pub fn popup_closed(id: RequestId) {
    let callback = PENDING.with_borrow_mut(|pending| {
        if matches!(pending.get(&id), Some(Completion::Popup(_))) {
            pending.remove(&id)
        } else {
            None
        }
    });
    if let Some(Completion::Popup(callback)) = callback {
        report(callback(id));
    }
}

/// Add items to the open picker `id` (from [`pick`]), e.g. as a program's
/// output arrives. Fails if the picker was closed.
pub fn push_items(id: RequestId, items: Vec<Item>) -> Result<(), String> {
    push_items_inner(id, None, items)
}

fn push_items_inner(
    id: RequestId,
    generation: Option<u64>,
    items: Vec<Item>,
) -> Result<(), String> {
    #[cfg(feature = "api-v0-2")]
    {
        let _ = generation;
        ui::push_items(id, &items)?;
    }
    #[cfg(not(feature = "api-v0-2"))]
    ui::push_items(id, generation, &items)?;
    PICKER_ITEMS.with_borrow_mut(|open| {
        if let Some(open) = open.get_mut(&id) {
            COMMAND.with_borrow_mut(|transaction| {
                if let Some(transaction) = transaction
                    .as_mut()
                    .filter(|transaction| transaction.existing.contains(&id))
                {
                    transaction
                        .items
                        .entry(id)
                        .or_insert(ItemUndo::Truncate(open.len()));
                }
            });
            open.extend(items);
        }
    });
    Ok(())
}

/// Append results belonging to one live query, dropping obsolete batches.
#[cfg(not(feature = "api-v0-2"))]
pub fn push_items_for_query(
    id: RequestId,
    generation: u64,
    items: Vec<Item>,
) -> Result<(), String> {
    if accepts_query(id, generation)? {
        push_items_inner(id, Some(generation), items)?;
    }
    Ok(())
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
        #[cfg(not(feature = "api-v0-2"))]
        STREAMS.with_borrow_mut(|streams| streams.remove(&id));
        report(callback(result));
    }
}

#[cfg(not(feature = "api-v0-2"))]
#[doc(hidden)]
pub fn process_output(id: RequestId, chunk: String) {
    if !PENDING.with_borrow(|pending| matches!(pending.get(&id), Some(Completion::Process(_)))) {
        return;
    }
    let Some(mut handler) = STREAMS.with_borrow_mut(|streams| streams.remove(&id)) else {
        return;
    };
    let result = handler(&chunk);
    if PENDING.with_borrow(|pending| matches!(pending.get(&id), Some(Completion::Process(_)))) {
        STREAMS.with_borrow_mut(|streams| streams.insert(id, handler));
    }
    report(result);
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

/// Run `task` with `input` in the plugin's background instance
/// ([`crate::Plugin::run_background`]): on another thread, with up to 30 s
/// and no editor access, so heavy work doesn't freeze the editor. `callback`
/// gets what it returned. At most 4 at once.
#[cfg(not(feature = "api-v0-2"))]
pub fn run_background(
    task: &str,
    input: impl Into<String>,
    callback: impl FnOnce(Result<String, String>) -> Result<(), String> + 'static,
) -> Result<RequestId, String> {
    let id = crate::bindings::helix::plugin::worker::run_background(task, &input.into())?;
    PENDING.with_borrow_mut(|pending| {
        pending.insert(id, Completion::Background(Box::new(callback)));
    });
    Ok(id)
}

#[doc(hidden)]
#[cfg(not(feature = "api-v0-2"))]
pub fn background_done(id: RequestId, result: Result<String, String>) {
    let callback = PENDING.with_borrow_mut(|pending| {
        if matches!(pending.get(&id), Some(Completion::Background(_))) {
            pending.remove(&id)
        } else {
            None
        }
    });
    if let Some(Completion::Background(callback)) = callback {
        report(callback(result));
    }
}

#[doc(hidden)]
pub fn prompt_done(id: RequestId, answer: Option<String>) {
    let callback = PENDING.with_borrow_mut(|pending| {
        if matches!(pending.get(&id), Some(Completion::Prompt(_))) {
            pending.remove(&id)
        } else {
            None
        }
    });
    if let Some(Completion::Prompt(callback)) = callback {
        report(callback(answer));
    }
}

#[doc(hidden)]
#[cfg(feature = "api-v0-2")]
pub fn picker_query(id: RequestId, query: String) {
    // Taken out while it runs, so it may use the registry itself.
    let Some(mut handler) = QUERIES.with_borrow_mut(|queries| queries.remove(&id)) else {
        return;
    };
    let result = handler(id, &query).and_then(|items| match items {
        Some(items) => set_items(id, items),
        None => Ok(()),
    });
    // Put back unless the picker was closed in the meantime.
    if PICKER_ITEMS.with_borrow(|open| open.contains_key(&id)) {
        QUERIES.with_borrow_mut(|queries| queries.insert(id, handler));
    }
    report(result);
}

#[cfg(not(feature = "api-v0-2"))]
#[doc(hidden)]
pub fn picker_query(id: RequestId, generation: u64, query: String) {
    let Some(changed) = GENERATIONS.with_borrow_mut(|generations| {
        let current = generations.get_mut(&id)?;
        let changed = *current != generation;
        *current = generation;
        Some(changed)
    }) else {
        return;
    };
    if changed {
        PICKER_ITEMS.with_borrow_mut(|open| {
            if let Some(items) = open.get_mut(&id) {
                items.clear();
            }
        });
    }
    let Some(mut handler) = QUERIES.with_borrow_mut(|queries| queries.remove(&id)) else {
        return;
    };
    let result = handler(id, generation, &query).and_then(|items| match items {
        Some(items) => set_items_for_query(id, generation, items),
        None => Ok(()),
    });
    if PICKER_ITEMS.with_borrow(|open| open.contains_key(&id)) {
        QUERIES.with_borrow_mut(|queries| queries.insert(id, handler));
    }
    report(result);
}

pub(crate) fn run_command(f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    // Only the outer command export is a host transaction. A nested Rust
    // helper still belongs to that same call.
    if COMMAND.with_borrow(Option::is_some) {
        return f();
    }
    let existing = PENDING.with_borrow(|pending| pending.keys().copied().collect());
    COMMAND.with_borrow_mut(|command| {
        *command = Some(CommandTransaction {
            existing,
            items: HashMap::new(),
        })
    });
    let result = f();
    if let Some(transaction) = COMMAND.with_borrow_mut(Option::take) {
        if result.is_err() {
            PICKER_ITEMS.with_borrow_mut(|open| {
                for (id, undo) in transaction.items {
                    if let Some(items) = open.get_mut(&id) {
                        match undo {
                            ItemUndo::Truncate(len) => items.truncate(len),
                            ItemUndo::Replace(previous) => *items = previous,
                        }
                    }
                }
                open.retain(|id, _| transaction.existing.contains(id));
            });
            QUERIES.with_borrow_mut(|queries| {
                queries.retain(|id, _| transaction.existing.contains(id))
            });
            #[cfg(not(feature = "api-v0-2"))]
            {
                GENERATIONS.with_borrow_mut(|generations| {
                    generations.retain(|id, _| transaction.existing.contains(id))
                });
                STREAMS.with_borrow_mut(|streams| {
                    streams.retain(|id, _| transaction.existing.contains(id))
                });
            }
            PENDING.with_borrow_mut(|pending| {
                pending.retain(|id, _| transaction.existing.contains(id))
            });
        }
    }
    result
}

/// Keep one undo entry per pre-existing picker, moving replacement lists
/// rather than cloning them. New requests are removed wholesale on failure.
fn record_replacement(id: RequestId, mut previous: Vec<Item>) {
    COMMAND.with_borrow_mut(|transaction| {
        let Some(transaction) = transaction
            .as_mut()
            .filter(|transaction| transaction.existing.contains(&id))
        else {
            return;
        };
        use std::collections::hash_map::Entry;
        match transaction.items.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(ItemUndo::Replace(previous));
            }
            Entry::Occupied(mut entry) => {
                if let ItemUndo::Truncate(len) = entry.get() {
                    previous.truncate(*len);
                    entry.insert(ItemUndo::Replace(previous));
                }
            }
        }
    });
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

    #[cfg(not(feature = "api-v0-2"))]
    #[test]
    fn streaming_callbacks_remain_open_until_the_final_completion() {
        let chunks = Rc::new(RefCell::new(String::new()));
        let completed = Rc::new(Cell::new(0));
        let count = completed.clone();
        register_process(20, move |result| {
            assert_eq!(result.unwrap().stdout, "tail");
            count.set(count.get() + 1);
            Ok(())
        });
        let output = chunks.clone();
        STREAMS.with_borrow_mut(|streams| {
            streams.insert(
                20,
                Box::new(move |chunk| {
                    output.borrow_mut().push_str(chunk);
                    Ok(())
                }),
            )
        });
        process_output(20, "one\n".into());
        process_output(20, "two\n".into());
        assert_eq!(&*chunks.borrow(), "one\ntwo\n");
        assert_eq!(completed.get(), 0);
        process_done(
            20,
            Ok(Output {
                exit_code: Some(0),
                stdout: "tail".into(),
                stderr: String::new(),
            }),
        );
        process_output(20, "late".into());
        process_done(20, Err("duplicate".into()));
        assert_eq!(completed.get(), 1);
        assert_eq!(&*chunks.borrow(), "one\ntwo\n");
    }

    #[cfg(not(feature = "api-v0-2"))]
    #[test]
    fn popup_completion_is_one_shot_and_checks_its_kind() {
        let count = Rc::new(Cell::new(0));
        let received = count.clone();
        PENDING.with_borrow_mut(|pending| {
            pending.insert(
                40,
                Completion::Popup(Box::new(move |id| {
                    assert_eq!(id, 40);
                    received.set(received.get() + 1);
                    Ok(())
                })),
            )
        });
        process_done(40, Err("wrong kind".into()));
        popup_closed(40);
        popup_closed(40);
        assert_eq!(count.get(), 1);
    }

    #[cfg(not(feature = "api-v0-2"))]
    #[test]
    fn a_new_query_clears_local_rows_and_old_batches_never_call_the_host() {
        PICKER_ITEMS.with_borrow_mut(|open| open.insert(30, vec![Item::text("old")]));
        GENERATIONS.with_borrow_mut(|generations| generations.insert(30, 1));
        QUERIES.with_borrow_mut(|queries| {
            queries.insert(
                30,
                Box::new(|id, generation, query| {
                    assert_eq!((id, generation, query), (30, 2, "new"));
                    assert!(PICKER_ITEMS.with_borrow(|open| open[&id].is_empty()));
                    Ok(None)
                }),
            )
        });
        picker_query(30, 2, "new".into());
        // Native test stubs cannot call WIT; stale helpers must return first.
        push_items_for_query(30, 1, vec![Item::text("stale")]).unwrap();
        set_items_for_query(30, 1, vec![Item::text("stale")]).unwrap();
        assert!(PICKER_ITEMS.with_borrow(|open| open[&30].is_empty()));
        PICKER_ITEMS.with_borrow_mut(|open| open.remove(&30));
        GENERATIONS.with_borrow_mut(|generations| generations.remove(&30));
        QUERIES.with_borrow_mut(|queries| queries.remove(&30));
    }

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
