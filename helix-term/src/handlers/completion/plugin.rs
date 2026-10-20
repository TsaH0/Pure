//! Completions from plugins (Pure) that registered as completion sources.
use std::{borrow::Cow, sync::Arc, time::Duration};

use helix_core::{self as core, chars::char_is_word, completion::CompletionProvider, Transaction};
use helix_view::{document::SavePoint, handlers::completion::ResponseContext, Editor};

use super::{CompletionItem, CompletionItems, CompletionResponse};

/// How long the menu waits for a plugin's answer.
const TIMEOUT: Duration = Duration::from_secs(1);

pub(super) fn completions(
    editor: &mut Editor,
    savepoint: Arc<SavePoint>,
) -> Vec<impl std::future::Future<Output = CompletionResponse>> {
    let requests = crate::plugins::request_completions(editor);
    if requests.is_empty() {
        return Vec::new();
    }
    let (view, doc) = current_ref!(editor);
    let rope = doc.text().clone();
    let selection = doc.selection(view.id).clone();
    let text = rope.slice(..);
    let cursor = selection.primary().cursor(text);
    // By default the word before the cursor is replaced.
    let word_start = cursor
        - text
            .chars_at(cursor)
            .reversed()
            .take_while(|c| char_is_word(*c))
            .count();
    requests
        .into_iter()
        .map(|(index, receiver)| {
            let rope = rope.clone();
            let selection = selection.clone();
            let savepoint = savepoint.clone();
            async move {
                let answer = tokio::time::timeout(TIMEOUT, receiver)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let provider = CompletionProvider::Plugin(index);
                let items = answer
                    .into_iter()
                    .map(|item| {
                        let from = item.replace_from.filter(|from| *from <= cursor);
                        let replaced = cursor - from.unwrap_or(word_start);
                        let insert = item.insert.unwrap_or_else(|| item.label.clone());
                        let text = rope.slice(..);
                        let transaction =
                            Transaction::change_by_selection(&rope, &selection, |range| {
                                let cursor = range.cursor(text);
                                (
                                    cursor.saturating_sub(replaced),
                                    cursor,
                                    Some(insert.as_str().into()),
                                )
                            });
                        CompletionItem::Other(core::CompletionItem {
                            transaction,
                            label: item.label.into(),
                            kind: item.kind.map_or(Cow::Borrowed("plugin"), Cow::Owned),
                            documentation: item.documentation,
                            provider,
                        })
                    })
                    .collect();
                CompletionResponse {
                    items: CompletionItems::Other(items),
                    provider,
                    context: ResponseContext {
                        is_incomplete: false,
                        priority: 0,
                        savepoint,
                    },
                }
            }
        })
        .collect()
}
