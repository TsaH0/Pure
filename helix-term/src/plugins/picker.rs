//! Pickers shown by plugins (`show-picker`, `push-items`, `set-items`, live
//! pickers).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

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
    columns: Vec<String>,
    /// A file to preview, with lines (0-based, inclusive) to highlight.
    preview: Option<(PathBuf, Option<(usize, usize)>)>,
}

/// Whose picker it is.
pub struct Data {
    plugin: String,
    id: u32,
    /// Items so far, to number pushed ones.
    len: AtomicU32,
}

type PluginPicker = Picker<Item, Data>;

fn item(index: u32, item: PickerItem) -> Item {
    let preview = match item.preview {
        Some(PickerPreview::File((path, lines))) => Some((
            PathBuf::from(path),
            lines.map(|(from, to)| (from as usize, to as usize)),
        )),
        // Text previews need a document to show them in; not supported yet.
        Some(PickerPreview::Text(_)) | None => None,
    };
    Item {
        index,
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
        },
    );
    async { Ok(()) }.boxed()
}

/// Show the picker `spec` for `plugin`'s request `id`.
pub fn show(compositor: &mut Compositor, plugin: String, id: u32, spec: PickerSpec) {
    let len = spec.items.len() as u32;
    let items: Vec<Item> = spec
        .items
        .into_iter()
        .enumerate()
        .map(|(index, it)| item(index as u32, it))
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
    };
    let on_accept = plugin.clone();
    let on_close = plugin.clone();
    let mut picker = Picker::new(
        columns,
        primary_column,
        items,
        data,
        move |cx, item: &Item, action| {
            super::deliver(
                cx.editor,
                cx.jobs,
                &on_accept,
                Callback::PickerDone {
                    id,
                    result: PickerResult::Accepted((vec![item.index], open_action(action))),
                },
            );
        },
    )
    .with_preview(|_editor, item: &Item| {
        item.preview
            .as_ref()
            .map(|(path, lines)| (path.as_path().into(), *lines))
    })
    .with_on_close(move |cx| {
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
    // Its request is answered once; choosing again would go nowhere.
    .not_reopenable();
    if let Some(initial) = spec.initial.filter(|&initial| initial < len) {
        picker = picker.with_initial_cursor(initial);
    }
    if live {
        // Sends the (empty) initial query right away.
        picker = picker.with_dynamic_query(live_query, Some(100));
    }
    compositor.push(Box::new(ui::overlay::overlaid(picker)));
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
    for it in items {
        if injector
            .push(item(len.fetch_add(1, Ordering::Relaxed), it))
            .is_err()
        {
            break;
        }
    }
}

/// Add items to `plugin`'s open picker `id`.
pub fn push_items(compositor: &mut Compositor, plugin: &str, id: u32, items: Vec<PickerItem>) {
    if let Some(picker) = find(compositor, plugin, id) {
        push(picker, items);
    }
}

/// Replace the items of `plugin`'s open picker `id`.
pub fn set_items(compositor: &mut Compositor, plugin: &str, id: u32, items: Vec<PickerItem>) {
    if let Some(picker) = find(compositor, plugin, id) {
        drop(picker.restart());
        picker.editor_data().len.store(0, Ordering::Relaxed);
        push(picker, items);
    }
}
