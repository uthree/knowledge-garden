//! Markdown to safe HTML. Entries and comments are written by other people and
//! agents, so the output is always sanitized: raw HTML in the source is reduced
//! to a small allow-list, scripts and event handlers are removed, and links
//! open without passing on the page.

use std::sync::LazyLock;

use maud::{Markup, PreEscaped};

use crate::files::Preview;
use crate::model::Attachment;
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
#[cfg(test)]
pub fn render(src: &str) -> Markup {
    render_with(src, "", &[])
}

/// The path a page uses for an attachment (relative, so it works under any host name).
pub fn file_path(entry_id: &str, name: &str) -> String {
    format!("/entries/{entry_id}/files/{}", crate::files::encode_path_segment(name))
}

const SCHEME: &str = "attachment:";

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Inline player or viewer for an attachment. Built from trusted parts only:
/// the path is percent-encoded and the caption is escaped.
pub fn embed_html(entry_id: &str, a: &Attachment, caption: &str) -> String {
    let path = escape(&file_path(entry_id, &a.name));
    let caption = escape(if caption.trim().is_empty() { &a.name } else { caption });
    let link = format!("<span class=\"kg-embed__caption\"><a href=\"{path}\">{caption}</a></span>");
    match a.preview {
        Preview::Image => format!(
            "<span class=\"kg-embed kg-embed--image\"><a href=\"{path}\"><img src=\"{path}\" alt=\"{caption}\" loading=\"lazy\"></a></span>"
        ),
        Preview::Audio => format!(
            "<span class=\"kg-embed kg-embed--audio\"><audio controls preload=\"metadata\" src=\"{path}\"></audio>{link}</span>"
        ),
        Preview::Video => format!(
            "<span class=\"kg-embed kg-embed--video\"><video controls preload=\"metadata\" src=\"{path}\"></video>{link}</span>"
        ),
        Preview::Pdf => format!(
            "<span class=\"kg-embed kg-embed--pdf\"><iframe src=\"{path}\" title=\"{caption}\" loading=\"lazy\"></iframe>{link}</span>"
        ),
        Preview::Text => format!(
            "<span class=\"kg-embed kg-embed--text\"><iframe src=\"{path}\" title=\"{caption}\" loading=\"lazy\"></iframe>{link}</span>"
        ),
        Preview::None => format!("<a class=\"kg-file-link\" href=\"{path}?download=1\">{caption}</a>"),
    }
}

fn missing_html(name: &str) -> String {
    format!(
        "<span class=\"kg-embed kg-embed--missing\">添付ファイル「{}」はありません</span>",
        escape(name)
    )
}

/// Renders Markdown whose `attachment:<name>` references point at the files of
/// entry `entry_id`. `![caption](attachment:plot.png)` shows an image, audio or
/// video player, or PDF viewer; `[text](attachment:run.log)` links to the file.
pub fn render_with(src: &str, entry_id: &str, attachments: &[Attachment]) -> Markup {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let find = |dest: &str| -> (String, Option<&Attachment>) {
        let name = percent_decode(&dest[SCHEME.len()..]);
        let a = attachments.iter().find(|a| a.name == name);
        (name, a)
    };
    // Players and viewers are not allowed through the sanitizer (people cannot write
    // them as raw HTML), so they are put in as placeholders and swapped in afterwards.
    let nonce: u64 = rand::random();
    let placeholder = |n: usize| format!("\u{E000}{nonce:016x}-{n}\u{E001}");
    let mut embeds: Vec<String> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut parser = Parser::new_ext(src, options);
    while let Some(event) = parser.next() {
        match event {
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) if dest_url.starts_with(SCHEME) => {
                // Collect the alt text up to the end of this image.
                let mut alt = String::new();
                let mut depth = 1;
                for e in parser.by_ref() {
                    match e {
                        Event::Start(Tag::Image { .. }) => depth += 1,
                        Event::End(TagEnd::Image) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                        _ => {}
                    }
                }
                match find(&dest_url) {
                    (_, Some(a)) if a.preview == Preview::Image => {
                        events.push(Event::Start(Tag::Image {
                            link_type,
                            dest_url: file_path(entry_id, &a.name).into(),
                            title,
                            id,
                        }));
                        events.push(Event::Text(alt.into()));
                        events.push(Event::End(TagEnd::Image));
                    }
                    (_, Some(a)) => {
                        events.push(Event::Text(placeholder(embeds.len()).into()));
                        embeds.push(embed_html(entry_id, a, &alt));
                    }
                    (name, None) => {
                        events.push(Event::Text(placeholder(embeds.len()).into()));
                        embeds.push(missing_html(&name));
                    }
                }
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) if dest_url.starts_with(SCHEME) => {
                let (name, a) = find(&dest_url);
                let mut dest = file_path(entry_id, &name);
                if a.is_some_and(|a| a.preview == Preview::None) {
                    dest.push_str("?download=1");
                }
                events.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: dest.into(),
                    title,
                    id,
                }));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match &kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or(""),
                    CodeBlockKind::Indented => "",
                };
                events.push(Event::Html(
                    format!(
                        "<div class=\"at-markdown__code\"><div class=\"at-markdown__code-header\"><span>{}</span></div><pre><code>",
                        escape(lang)
                    )
                    .into(),
                ));
            }
            Event::End(TagEnd::CodeBlock) => events.push(Event::Html("</code></pre></div>".into())),
            Event::TaskListMarker(done) => events.push(Event::Html(
                if done {
                    "<span class=\"at-markdown__box is-done\">✓</span> "
                } else {
                    "<span class=\"at-markdown__box\"></span> "
                }
                .into(),
            )),
            other => events.push(other),
        }
    }
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, events.into_iter());
    let mut clean = SANITIZER.clean(&out).to_string();
    for (n, embed) in embeds.iter().enumerate() {
        clean = clean.replace(&placeholder(n), embed);
    }
    PreEscaped(clean)
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

    fn att(name: &str, ct: &str) -> Attachment {
        Attachment {
            name: name.into(),
            content_type: ct.into(),
            size: 1,
            sha256: "0".repeat(64),
            preview: Preview::of(ct),
            url: String::new(),
            markdown: String::new(),
            posted_via: crate::model::PostedVia {
                name: "t".into(),
                kind: crate::model::TokenKind::Human,
            },
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn embeds_attachments() {
        let files = [
            att("plot.png", "image/png"),
            att("voice.mp3", "audio/mpeg"),
            att("run.mp4", "video/mp4"),
            att("paper.pdf", "application/pdf"),
            att("data.bin", "application/octet-stream"),
            att("損失.png", "image/png"),
        ];
        let html = render_with(
            "![Loss](attachment:plot.png)\n\n![](attachment:voice.mp3)\n\n![clip & \"more\"](attachment:run.mp4)\n\n\
             ![論文](attachment:paper.pdf)\n\n[raw](attachment:data.bin)\n\n![](attachment:%E6%90%8D%E5%A4%B1.png)\n\n\
             ![](attachment:nothing.png)\n\n<video src=x></video><iframe src=https://evil.example></iframe>",
            "E1",
            &files,
        )
        .0;
        assert!(
            html.contains("<img src=\"/entries/E1/files/plot.png\" alt=\"Loss\""),
            "{html}"
        );
        assert!(
            html.contains("<audio controls preload=\"metadata\" src=\"/entries/E1/files/voice.mp3\">"),
            "{html}"
        );
        assert!(
            html.contains("<video controls preload=\"metadata\" src=\"/entries/E1/files/run.mp4\">"),
            "{html}"
        );
        assert!(html.contains("clip &amp; &quot;more&quot;"), "{html}");
        assert!(
            html.contains("<iframe src=\"/entries/E1/files/paper.pdf\" title=\"論文\""),
            "{html}"
        );
        assert!(
            html.contains("href=\"/entries/E1/files/data.bin?download=1\""),
            "{html}"
        );
        assert!(html.contains("/entries/E1/files/%E6%90%8D%E5%A4%B1.png"), "{html}");
        assert!(html.contains("添付ファイル「nothing.png」はありません"), "{html}");
        // People still cannot write players or frames themselves.
        assert!(!html.contains("evil.example"), "{html}");
        assert!(!html.contains("<video src"), "{html}");
        assert!(!html.contains('\u{E000}'), "{html}");
    }
}
