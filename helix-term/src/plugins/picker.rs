//! Pickers shown by plugins (`show-picker`, `push-items`, `set-items`, live
//! pickers).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use helix_event::runtime_local;
use parking_lot::Mutex;

use futures_util::future::{BoxFuture, FutureExt};
use helix_plugin::{Callback, OpenAction, PickerItem, PickerPreview, PickerResult, PickerSpec};
use helix_view::editor::Action;
use helix_view::Editor;

use crate::compositor::Compositor;
use crate::ui::menu::Cell;
use crate::ui::overlay::Overlay;
use crate::ui::{self, picker::Injector, Picker, PickerColumn};

/// A row of a plugin picker.
pub struct Item {
    /// Index in the plugin's list (pushed items continue the count).
    index: u32,
    /// Which of the plugin's `set-items` lists the row is from.
    revision: u64,
    columns: Vec<String>,
    preview: Option<Preview>,
}

enum Preview {
    /// A file, with lines (0-based, inclusive) to highlight.
    File(PathBuf, Option<(usize, usize)>),
    Text(String),
}

/// Whose picker it is.
pub struct Data {
    plugin: String,
    id: u32,
    /// Items so far, to number pushed ones.
    len: AtomicU32,
    generation: Option<AtomicU64>,
    /// The `set-items` list shown (see `REQUESTED`).
    revision: AtomicU64,
}

type PluginPicker = Picker<Item, Data>;

runtime_local! {
    /// `set-items` calls per open picker, counted as soon as the plugin makes
    /// them. The plugin's list changes then; the picker only when the queued
    /// UI work runs. A row from an older list must not be chosen: its index
    /// would mean another item to the plugin.
    static REQUESTED: Mutex<BTreeMap<(String, u32), u64>> = Mutex::new(BTreeMap::new());
}

/// The plugin called `set-items` on picker `id`: the revision its new list is.
pub fn set_items_requested(plugin: &str, id: u32) -> u64 {
    let mut requested = REQUESTED.lock();
    let revision = requested.entry((plugin.to_string(), id)).or_default();
    *revision += 1;
    *revision
}

/// Whether the plugin's list is still the one `item` came from.
fn is_current(plugin: &str, id: u32, item: &Item) -> bool {
    let requested = REQUESTED.lock();
    requested
        .get(&(plugin.to_string(), id))
        .copied()
        .unwrap_or(0)
        == item.revision
}

fn forget(plugin: &str, id: u32) {
    REQUESTED.lock().remove(&(plugin.to_string(), id));
}

fn item(index: u32, revision: u64, item: PickerItem) -> Item {
    let preview = item.preview.map(|preview| match preview {
        PickerPreview::File((path, lines)) => Preview::File(
            PathBuf::from(path),
            lines.map(|(from, to)| (from as usize, to as usize)),
        ),
        PickerPreview::Text(text) => Preview::Text(text),
    });
    Item {
        index,
        revision,
        columns: item.columns,
        preview,
    }
}

/// The text of column `I`. Column formatters are plain functions, so there is
/// one per column.
fn column<'a, const I: usize>(item: &'a Item, _: &'a Data) -> Cell<'a> {
    item.columns.get(I).map_or("", String::as_str).into()
}

fn picker_column(index: usize, name: String) -> PickerColumn<Item, Data> {
    match index {
        0 => PickerColumn::new(name, column::<0>),
        1 => PickerColumn::new(name, column::<1>),
        2 => PickerColumn::new(name, column::<2>),
        3 => PickerColumn::new(name, column::<3>),
        4 => PickerColumn::new(name, column::<4>),
        5 => PickerColumn::new(name, column::<5>),
        6 => PickerColumn::new(name, column::<6>),
        // The host allows at most `MAX_PICKER_COLUMNS` (8).
        _ => PickerColumn::new(name, column::<7>),
    }
}

fn open_action(action: Action) -> OpenAction {
    match action {
        Action::HorizontalSplit => OpenAction::Hsplit,
        Action::VerticalSplit => OpenAction::Vsplit,
        _ => OpenAction::Replace,
    }
}

/// A live picker's query changed: the picker dropped its items. Tell the
/// plugin, which answers with `set-items` or `push-items`.
fn live_query(
    query: &str,
    _editor: &mut Editor,
    data: Arc<Data>,
    _injector: &Injector<Item, Data>,
) -> BoxFuture<'static, anyhow::Result<()>> {
    data.len.store(0, Ordering::Relaxed);
    super::deliver_later(
        data.plugin.clone(),
        Callback::PickerQuery {
            id: data.id,
            query: query.to_owned(),
            generation: data
                .generation
                .as_ref()
                .map(|generation| generation.load(Ordering::Relaxed)),
        },
    );
    async { Ok(()) }.boxed()
}

/// Show the picker `spec` for `plugin`'s request `id`.
pub fn show(
    compositor: &mut Compositor,
    plugin: String,
    id: u32,
    spec: PickerSpec,
    generation: Option<u64>,
) {
    // One plugin picker at a time: the previous one is cancelled. (Pickers
    // are found by type, so two would get each other's items and queries.)
    if let Some(Overlay { content, .. }) = compositor.find::<Overlay<PluginPicker>>() {
        let previous = content.editor_data();
        super::deliver_later(
            previous.plugin.clone(),
            Callback::PickerDone {
                id: previous.id,
                result: PickerResult::Cancelled,
            },
        );
        compositor.remove_type::<Overlay<PluginPicker>>();
    }
    let len = spec.items.len() as u32;
    let items: Vec<Item> = spec
        .items
        .into_iter()
        .enumerate()
        .map(|(index, it)| item(index as u32, 0, it))
        .collect();
    let live = spec.live;
    let mut columns: Vec<_> = spec
        .columns
        .into_iter()
        .enumerate()
        .map(|(index, name)| picker_column(index, name))
        .collect();
    // In a live picker what the user types goes to the plugin (as the query of
    // a hidden column), not to the matcher.
    let primary_column = if live {
        columns.push(PickerColumn::hidden("query"));
        columns.len() - 1
    } else {
        0
    };
    let data = Data {
        plugin: plugin.clone(),
        id,
        len: AtomicU32::new(len),
        generation: generation.map(AtomicU64::new),
        // Request ids are never reused: a new picker starts at revision 0.
        revision: AtomicU64::new(0),
    };
    let on_accept = plugin.clone();
    let on_close = plugin.clone();
    let mut picker = Picker::new(
        columns,
        primary_column,
        items,
        data,
        move |cx, item: &Item, action| {
            let result = if is_current(&on_accept, id, item) {
                PickerResult::Accepted((vec![item.index], open_action(action)))
            } else {
                cx.editor
                    .set_error("The plugin was replacing the picker's items: choose again");
                PickerResult::Cancelled
            };
            forget(&on_accept, id);
            super::deliver(
                cx.editor,
                cx.jobs,
                &on_accept,
                Callback::PickerDone { id, result },
            );
        },
    )
    .with_preview(|_editor, item: &Item| match item.preview.as_ref()? {
        Preview::File(path, lines) => Some((path.as_path().into(), *lines)),
        Preview::Text(text) => Some((ui::picker::PathOrId::Text(text), None)),
    })
    .with_on_close(move |cx| {
        forget(&on_close, id);
        super::deliver(
            cx.editor,
            cx.jobs,
            &on_close,
            Callback::PickerDone {
                id,
                result: PickerResult::Cancelled,
            },
        );
    })
    .with_title(spec.title)
    // Its request is answered once; choosing again would go nowhere.
    .not_reopenable();
    if let Some(initial) = spec.initial.filter(|&initial| initial < len) {
        picker = picker.with_initial_cursor(initial);
    }
    if live {
        if generation.is_some() {
            picker = picker.with_query_generations(query_changed);
        }
        // The plugin hears about the query once the user types; until then
        // the picker shows the initial items.
        picker = picker.with_dynamic_query(live_query, Some(100));
    }
    compositor.push(Box::new(ui::overlay::overlaid(picker)));
}

fn query_changed(picker: &mut PluginPicker, generation: u64) {
    let data = picker.editor_data();
    data.len.store(0, Ordering::Relaxed);
    if let Some(current) = &data.generation {
        current.store(generation, Ordering::Relaxed);
    }
    super::with_host(|host| host.set_picker_generation(&data.plugin, data.id, generation));
}

/// `plugin`'s open picker `id`, if it is still open.
fn find<'a>(compositor: &'a mut Compositor, plugin: &str, id: u32) -> Option<&'a mut PluginPicker> {
    let Overlay {
        content: picker, ..
    } = compositor.find::<Overlay<PluginPicker>>()?;
    let data = picker.editor_data();
    (data.plugin == plugin && data.id == id).then_some(picker)
}

fn push(picker: &PluginPicker, items: Vec<PickerItem>) {
    // A fresh injector each time: one kept around would show the picker as
    // "running" for good.
    let injector = picker.injector();
    let len = &picker.editor_data().len;
    let revision = picker.editor_data().revision.load(Ordering::Relaxed);
    for it in items {
        if injector
            .push(item(len.fetch_add(1, Ordering::Relaxed), revision, it))
            .is_err()
        {
            break;
        }
    }
}

/// Add items to `plugin`'s open picker `id`.
pub fn push_items(
    compositor: &mut Compositor,
    plugin: &str,
    id: u32,
    generation: Option<u64>,
    items: Vec<PickerItem>,
) {
    if let Some(picker) = find(compositor, plugin, id) {
        if current_generation(picker) == generation {
            push(picker, items);
        }
    }
}

/// Replace the items of `plugin`'s open picker `id`.
pub fn set_items(
    compositor: &mut Compositor,
    plugin: &str,
    id: u32,
    generation: Option<u64>,
    revision: u64,
    items: Vec<PickerItem>,
) {
    if let Some(picker) = find(compositor, plugin, id) {
        // The plugin's list is this one now, even if it is a stale query's.
        picker
            .editor_data()
            .revision
            .store(revision, Ordering::Relaxed);
        if current_generation(picker) != generation {
            return;
        }
        picker.clear_items();
        picker.editor_data().len.store(0, Ordering::Relaxed);
        push(picker, items);
    }
}

fn current_generation(picker: &PluginPicker) -> Option<u64> {
    picker
        .editor_data()
        .generation
        .as_ref()
        .map(|generation| generation.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::graphics::Rect;

    #[tokio::test]
    async fn queued_stale_batches_cannot_change_the_current_picker() {
        let mut compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        show(
            &mut compositor,
            "generation-test".into(),
            123,
            PickerSpec {
                title: "Live".into(),
                columns: vec!["Name".into()],
                items: vec![],
                initial: None,
                live: true,
            },
            Some(0),
        );
        let row = || PickerItem {
            columns: vec!["current".into()],
            data: "current".into(),
            preview: None,
        };
        query_changed(find(&mut compositor, "generation-test", 123).unwrap(), 1);
        set_items(
            &mut compositor,
            "generation-test",
            123,
            Some(1),
            1,
            vec![row()],
        );
        set_items(&mut compositor, "generation-test", 123, Some(0), 2, vec![]);
        push_items(
            &mut compositor,
            "generation-test",
            123,
            Some(0),
            vec![row(), row()],
        );
        assert_eq!(
            find(&mut compositor, "generation-test", 123)
                .unwrap()
                .editor_data()
                .len
                .load(Ordering::Relaxed),
            1
        );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn row(revision: u64) -> Item {
        Item {
            index: 0,
            revision,
            columns: Vec::new(),
            preview: None,
        }
    }

    #[test]
    fn rows_from_replaced_lists_are_not_chosen() {
        let (plugin, id) = ("revisions-test", 7);
        forget(plugin, id);
        // Before any set-items: the initial rows are current.
        assert!(is_current(plugin, id, &row(0)));
        // The plugin replaced its list; the picker hasn't caught up yet.
        let revision = set_items_requested(plugin, id);
        assert_eq!(revision, 1);
        assert!(!is_current(plugin, id, &row(0)));
        // Rows of the new list (stamped when the queued update runs).
        assert!(is_current(plugin, id, &row(1)));
        assert_eq!(set_items_requested(plugin, id), 2);
        assert!(!is_current(plugin, id, &row(1)));
        // Closing the picker forgets it.
        forget(plugin, id);
        assert!(is_current(plugin, id, &row(0)));
    }
}
