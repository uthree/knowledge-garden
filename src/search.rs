//! Search query parsing and result decoration. Matching is purely string based:
//! SQLite FTS5 with the trigram tokenizer, substring matching for short terms,
//! and regular expressions. No learned or vector models are involved.

use regex::{Regex, RegexBuilder};

/// Terms with at least this many characters use the trigram index; shorter ones
/// (common in Japanese, e.g. "検索") fall back to substring matching.
pub const MIN_INDEXED_CHARS: usize = 3;
pub const MAX_TERMS: usize = 16;
pub const MAX_TERM_CHARS: usize = 200;
pub const MAX_REGEX_CHARS: usize = 500;

/// A parsed search query. Syntax:
/// - whitespace (including full-width spaces) separates terms; all terms must match
/// - `"quoted phrase"` matches the phrase as one term
/// - `-term` or `-"phrase"` excludes entries containing it
#[derive(Debug, Default, PartialEq)]
pub struct ParsedQuery {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl ParsedQuery {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Included terms long enough for the trigram index.
    pub fn indexed_terms(&self) -> Vec<String> {
        self.include
            .iter()
            .filter(|t| t.chars().count() >= MIN_INDEXED_CHARS)
            .cloned()
            .collect()
    }

    /// Included terms too short for the trigram index.
    pub fn short_terms(&self) -> Vec<String> {
        self.include
            .iter()
            .filter(|t| t.chars().count() < MIN_INDEXED_CHARS)
            .cloned()
            .collect()
    }
}

pub fn parse_query(q: &str) -> Result<ParsedQuery, String> {
    let mut parsed = ParsedQuery::default();
    let mut chars = q.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else { break };
        let negate = first == '-';
        if negate {
            chars.next();
        }
        let mut term = String::new();
        if chars.next_if_eq(&'"').is_some() {
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                term.push(c);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                term.push(c);
            }
        }
        let term = term.trim().to_string();
        if term.is_empty() {
            continue;
        }
        if term.chars().count() > MAX_TERM_CHARS {
            return Err(format!("search terms must be at most {MAX_TERM_CHARS} characters"));
        }
        if negate {
            &mut parsed.exclude
        } else {
            &mut parsed.include
        }
        .push(term);
    }
    if parsed.include.len() + parsed.exclude.len() > MAX_TERMS {
        return Err(format!("at most {MAX_TERMS} search terms are allowed"));
    }
    if parsed.include.is_empty() && !parsed.exclude.is_empty() {
        return Err("a search needs at least one term that is not excluded".into());
    }
    Ok(parsed)
}

/// Builds an FTS5 MATCH expression requiring every term as a literal phrase.
pub fn fts_match_expr(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Builds a `LIKE` pattern (with `\` as the escape character) matching `term` anywhere.
pub fn like_pattern(term: &str) -> String {
    let mut p = String::from("%");
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            p.push('\\');
        }
        p.push(c);
    }
    p.push('%');
    p
}

pub fn compile_regex(pattern: &str) -> Result<Regex, String> {
    if pattern.chars().count() > MAX_REGEX_CHARS {
        return Err(format!("regex must be at most {MAX_REGEX_CHARS} characters"));
    }
    RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("invalid regex: {e}"))
}

/// Finds where results matched, for snippets and `matched_in`.
pub struct Highlighter {
    terms: Option<Regex>,
    regex: Option<Regex>,
}

impl Highlighter {
    pub fn new(include: &[String], regex: Option<Regex>) -> Self {
        let terms = (!include.is_empty()).then(|| {
            let alts: Vec<String> = include.iter().map(|t| regex::escape(t)).collect();
            RegexBuilder::new(&alts.join("|"))
                .case_insensitive(true)
                .build()
                .expect("escaped terms are valid")
        });
        Self { terms, regex }
    }

    fn first_match(&self, text: &str) -> Option<usize> {
        [&self.terms, &self.regex]
            .into_iter()
            .flatten()
            .filter_map(|r| r.find(text).map(|m| m.start()))
            .min()
    }

    pub fn matches(&self, text: &str) -> bool {
        self.first_match(text).is_some()
    }

    /// A short excerpt of `text` around the first match, on one line.
    pub fn snippet(&self, text: &str) -> Option<String> {
        const BEFORE: usize = 60;
        const AFTER: usize = 140;
        let pos = self.first_match(text)?;
        let start = text[..pos].char_indices().rev().nth(BEFORE - 1).map_or(0, |(i, _)| i);
        let end = text[pos..]
            .char_indices()
            .nth(AFTER)
            .map_or(text.len(), |(i, _)| pos + i);
        let mut s = String::new();
        if start > 0 {
            s.push('…');
        }
        s.push_str(&text[start..end].split_whitespace().collect::<Vec<_>>().join(" "));
        if end < text.len() {
            s.push('…');
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms_phrases_and_exclusions() {
        let q = parse_query(r#"rust  "query builder"　検索 -python -"old api""#).unwrap();
        assert_eq!(q.include, vec!["rust", "query builder", "検索"]);
        assert_eq!(q.exclude, vec!["python", "old api"]);
        assert_eq!(q.indexed_terms(), vec!["rust", "query builder"]);
        assert_eq!(q.short_terms(), vec!["検索"]);
    }

    #[test]
    fn rejects_only_exclusions() {
        assert!(parse_query("-foo").is_err());
        assert!(parse_query("   ").unwrap().is_empty());
    }

    #[test]
    fn escapes_fts_and_like() {
        assert_eq!(fts_match_expr(&["a\"b".into(), "cde".into()]), r#""a""b" AND "cde""#);
        assert_eq!(like_pattern("50%_a\\"), r"%50\%\_a\\%");
    }

    #[test]
    fn snippet_centers_on_first_match() {
        let h = Highlighter::new(&["needle".into()], None);
        let text = format!("{}NEEDLE\nafter", "x".repeat(100));
        let s = h.snippet(&text).unwrap();
        assert!(s.starts_with('…') && s.contains("NEEDLE after"), "{s}");
        assert!(h.snippet("nothing here").is_none());
    }

    #[test]
    fn snippet_handles_multibyte_text() {
        let h = Highlighter::new(&["検索".into()], None);
        let text = format!("{}全文検索の話{}", "あ".repeat(80), "い".repeat(200));
        let s = h.snippet(&text).unwrap();
        assert!(s.contains("全文検索の話") && s.ends_with('…'));
    }

    #[test]
    fn regex_is_size_limited() {
        assert!(compile_regex("a{1000}{1000}").is_err());
        assert!(compile_regex(r"error\[E\d+\]").is_ok());
    }
}
