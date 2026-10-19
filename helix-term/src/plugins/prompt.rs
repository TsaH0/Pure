//! Prompts shown by plugins (`show-prompt`).

use helix_plugin::{Callback, PromptSpec};

use helix_view::Editor;

use crate::compositor::Compositor;
use crate::ui::{Prompt, PromptEvent};

/// Ask for a line of text for `plugin`'s request `id`.
pub fn show(
    editor: &Editor,
    compositor: &mut Compositor,
    plugin: String,
    id: u32,
    spec: PromptSpec,
) {
    let completions = spec.completions;
    let message = if spec.message.ends_with(' ') {
        spec.message
    } else {
        format!("{} ", spec.message)
    };
    let mut prompt = Prompt::new(
        message.into(),
        None,
        move |_editor, input| {
            helix_core::fuzzy::fuzzy_match(input, completions.iter(), false)
                .into_iter()
                .map(|(completion, _)| (0.., completion.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            let answer = match event {
                PromptEvent::Validate => Some(input.to_owned()),
                PromptEvent::Abort => None,
                PromptEvent::Update => return,
            };
            super::deliver(
                cx.editor,
                cx.jobs,
                &plugin,
                Callback::PromptDone { id, result: answer },
            );
        },
    );
    if !spec.initial.is_empty() {
        prompt.set_line(spec.initial, editor);
    }
    prompt.recalculate_completion(editor);
    compositor.push(Box::new(prompt));
}
