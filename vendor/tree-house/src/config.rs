use once_cell::sync::Lazy;
use regex::Regex;
use tree_sitter::{query, Grammar};

use crate::highlighter::{Highlight, HighlightQuery};
use crate::injections_query::{InjectionLanguageMarker, InjectionsQuery};
use crate::Language;

use std::fmt::Write;
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug)]
pub struct LanguageConfig {
    pub grammar: Grammar,
    /// Shared by languages with the same grammar and queries (Rust and
    /// `rust-format-args-macro`, say), so it is compiled once.
    pub highlight_query: Arc<HighlightQuery>,
    pub injection_query: InjectionsQuery,
}

/// Compiled highlight queries by grammar and source (the highlights and
/// locals, trimmed: `; inherits` adds blank lines), while in use.
static HIGHLIGHT_QUERIES: Mutex<Vec<(Grammar, String, Weak<HighlightQuery>)>> =
    Mutex::new(Vec::new());

/// Queries at least this long are compiled on threads of their own, at the
/// same time; below it a thread costs more than it saves.
pub(crate) const PARALLEL_QUERY_LEN: usize = 4096;

/// Run `a` and `b`, at the same time when `parallel`.
pub(crate) fn join<A: Send, B: Send>(
    parallel: bool,
    a: impl FnOnce() -> A + Send,
    b: impl FnOnce() -> B,
) -> (A, B) {
    if !parallel {
        let a = a();
        return (a, b());
    }
    let result = std::thread::scope(|scope| {
        let a = scope.spawn(a);
        let b = b();
        let a = a
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (a, b)
    });
    // Give back the memory the other thread freed: glibc keeps it in that
    // thread's arena otherwise (megabytes after compiling a big query).
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: only releases free memory to the system.
        unsafe { malloc_trim(0) };
    }
    result
}

impl LanguageConfig {
    pub fn new(
        grammar: Grammar,
        highlight_query_text: &str,
        injection_query_text: &str,
        local_query_text: &str,
    ) -> Result<Self, query::ParseError> {
        // NOTE: the injection queries are parsed first since the local query is passed as-is
        // to `Query::new` in `InjectionsQuery::new`. This ensures that the more readable error
        // bubbles up first if the locals queries have an issue.
        let source = format!(
            "{}\n{}",
            highlight_query_text.trim(),
            local_query_text.trim()
        );
        let shared = {
            let mut queries = HIGHLIGHT_QUERIES.lock().unwrap();
            queries.retain(|(_, _, query)| query.strong_count() > 0);
            queries
                .iter()
                .find(|(other, text, _)| *other == grammar && *text == source)
                .and_then(|(_, _, query)| query.upgrade())
        };
        let parallel = shared.is_none() && source.len() >= PARALLEL_QUERY_LEN;
        let (highlight_query, injection_query) = join(
            parallel,
            || match shared {
                Some(query) => Ok(query),
                None => HighlightQuery::new(grammar, highlight_query_text, local_query_text)
                    .map(Arc::new),
            },
            || InjectionsQuery::new(grammar, injection_query_text, local_query_text),
        );
        let injection_query = injection_query?;
        let highlight_query = highlight_query?;
        let mut queries = HIGHLIGHT_QUERIES.lock().unwrap();
        if !queries
            .iter()
            .any(|(other, text, _)| *other == grammar && *text == source)
        {
            queries.push((grammar, source, Arc::downgrade(&highlight_query)));
        }
        drop(queries);

        Ok(Self {
            grammar,
            highlight_query,
            injection_query,
        })
    }

    pub fn configure(&self, mut f: impl FnMut(&str) -> Option<Highlight>) {
        self.highlight_query.configure(&mut f);
        self.injection_query.configure(&mut f);
    }
}

static INHERITS_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r";+\s*inherits\s*:?\s*([a-z_,()-]+)\s*").unwrap());

/// reads a query by invoking `read_query_text`, handles any `inherits` directives
pub fn read_query(language: &str, mut read_query_text: impl FnMut(&str) -> String) -> String {
    fn read_query_impl(language: &str, read_query_text: &mut impl FnMut(&str) -> String) -> String {
        let query = read_query_text(language);

        // replaces all "; inherits <language>(,<language>)*" with the queries of the given language(s)
        INHERITS_REGEX
            .replace_all(&query, |captures: &regex::Captures| {
                captures[1]
                    .split(',')
                    .fold(String::new(), |mut output, language| {
                        // `write!` to a String cannot fail.
                        write!(
                            output,
                            "\n{}\n",
                            read_query_impl(language, &mut *read_query_text)
                        )
                        .unwrap();
                        output
                    })
            })
            .into_owned()
    }
    read_query_impl(language, &mut read_query_text)
}

pub trait LanguageLoader {
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language>;
    fn get_config(&self, lang: Language) -> Option<&LanguageConfig>;
}

impl<T> LanguageLoader for &'_ T
where
    T: LanguageLoader,
{
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language> {
        T::language_for_marker(self, marker)
    }

    fn get_config(&self, lang: Language) -> Option<&LanguageConfig> {
        T::get_config(self, lang)
    }
}
