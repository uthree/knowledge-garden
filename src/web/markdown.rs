//! Markdown to safe HTML. Entries and comments are written by other people and
//! agents, so the output is always sanitized: raw HTML in the source is reduced
//! to a small allow-list, scripts and event handlers are removed, and links
//! open without passing on the page.

use std::sync::LazyLock;

use maud::{Markup, PreEscaped};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};

static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut b = ammonia::Builder::default();
    b.add_allowed_classes("div", ["at-markdown__code", "at-markdown__code-header"])
        .add_allowed_classes("span", ["at-markdown__box", "is-done"])
        .link_rel(Some("noopener noreferrer nofollow ugc"));
    b
});

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Renders Markdown as sanitized HTML inside `.at-markdown`.
pub fn render(src: &str) -> Markup {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let events = Parser::new_ext(src, options).map(|event| match event {
        Event::Start(Tag::CodeBlock(kind)) => {
            let lang = match &kind {
                CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or(""),
                CodeBlockKind::Indented => "",
            };
            Event::Html(
                format!(
                    "<div class=\"at-markdown__code\"><div class=\"at-markdown__code-header\"><span>{}</span></div><pre><code>",
                    escape(lang)
                )
                .into(),
            )
        }
        Event::End(TagEnd::CodeBlock) => Event::Html("</code></pre></div>".into()),
        Event::TaskListMarker(done) => Event::Html(
            if done {
                "<span class=\"at-markdown__box is-done\">✓</span> "
            } else {
                "<span class=\"at-markdown__box\"></span> "
            }
            .into(),
        ),
        other => other,
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, events);
    PreEscaped(SANITIZER.clean(&out).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_handlers() {
        let html =
            render("hi <script>alert(1)</script> <img src=x onerror=alert(1)> <a href=\"javascript:alert(1)\">x</a>").0;
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onerror"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
    }

    #[test]
    fn renders_code_blocks_tables_and_tasks() {
        let html =
            render("```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n").0;
        assert!(html.contains("<div class=\"at-markdown__code\">"), "{html}");
        assert!(html.contains("<span>rust</span>"), "{html}");
        assert!(html.contains("fn main() {}"), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("at-markdown__box is-done"), "{html}");
    }

    #[test]
    fn code_language_is_escaped() {
        let html = render("```\"><script>x</script>\nbody\n```\n").0;
        assert!(!html.contains("<script"), "{html}");
    }

    #[test]
    fn links_get_rel() {
        let html = render("[x](https://example.com)").0;
        assert!(html.contains("rel=\"noopener noreferrer nofollow ugc\""), "{html}");
    }
}
