//! Writing a new entry, or editing one: a new version for the content, and the metadata
//! (visibility, status, tags) changed in place.

use atelier::prelude::*;
use atelier::{
    Card, FormField, FormGroup, IconName, Markdown, SectionHeader, Segment, SegmentedControl, Tabs, TextInput, Tone,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, Window, div, px};
use knowledge_garden::model::{Entry, EntryKind, Status, Visibility};

use crate::api::{ApiError, NewEntry, Written};
use crate::app::{Nav, Session};
use crate::runtime;
use crate::widgets::{self, kind_icon, kind_label, status_label};

const VISIBILITIES: [(Visibility, &str); 5] = [
    (Visibility::Public, "公開"),
    (Visibility::Unlisted, "限定公開"),
    (Visibility::Federated, "連合"),
    (Visibility::Members, "メンバー"),
    (Visibility::Private, "非公開"),
];

fn visibility_help(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "誰でも読めます。一覧と検索に出て、連合先にもミラーされます。",
        Visibility::Unlisted => "URL を知っている人だけが読めます。一覧と検索には出ません。",
        Visibility::Federated => "この Garden のメンバーと、承認した連合先の Garden が読めます。",
        Visibility::Members => "この Garden のメンバーだけが読めます。",
        Visibility::Private => "自分だけが読めます。",
    }
}

/// Splits tags typed with spaces, commas or 、 between them.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == '、' || c == '，')
        .map(|t| t.trim_start_matches('#').trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    tags.dedup();
    tags
}

/// The recommended headings of `kind` as an empty body to fill in.
pub fn template(kind: EntryKind) -> String {
    kind.recommended_sections()
        .iter()
        .map(|s| format!("# {}\n\n", s.label.split(" / ").next().unwrap_or(s.label)))
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub struct ComposePage {
    session: Session,
    /// The entry being edited, once loaded.
    editing: Option<Entry>,
    edit_id: Option<String>,
    kind: EntryKind,
    visibility: Visibility,
    status: Option<Status>,
    title: Entity<TextInput>,
    summary: Entity<TextInput>,
    tags: Entity<TextInput>,
    body: Entity<TextInput>,
    preview: bool,
    saving: bool,
    error: Option<String>,
}

impl EventEmitter<Nav> for ComposePage {}

impl ComposePage {
    pub fn new(session: Session, edit: Option<String>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let visibility = session
            .garden
            .as_ref()
            .map(|g| g.default_visibility)
            .unwrap_or(Visibility::Public);
        let page = Self {
            session,
            editing: None,
            edit_id: edit.clone(),
            kind: EntryKind::Memo,
            visibility,
            status: None,
            title: widgets::input(cx),
            summary: widgets::input(cx),
            tags: widgets::input(cx),
            body: widgets::area_input("", cx),
            preview: false,
            saving: false,
            error: None,
        };
        if let Some(id) = edit {
            let client = page.session.client.clone();
            cx.spawn(async move |this, cx| {
                let entry = runtime::run(async move { client.entry(&id, None).await }).await;
                this.update(cx, |this, cx| {
                    match entry {
                        Ok(entry) => this.fill(entry, cx),
                        Err(e) => this.error = Some(e.to_string()),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        page
    }

    fn fill(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.kind = entry.kind;
        self.visibility = entry.visibility;
        self.status = Some(entry.status);
        self.title.update(cx, |i, cx| i.set_text(entry.title.clone(), cx));
        self.summary
            .update(cx, |i, cx| i.set_text(entry.summary.clone().unwrap_or_default(), cx));
        self.tags.update(cx, |i, cx| i.set_text(entry.tags.join(" "), cx));
        self.body.update(cx, |i, cx| i.set_text(entry.body.clone(), cx));
        self.editing = Some(entry);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let title = widgets::text(&self.title, cx);
        let summary = Some(widgets::text(&self.summary, cx)).filter(|s| !s.is_empty());
        let body = self.body.read(cx).text().to_string();
        let tags = parse_tags(self.tags.read(cx).text());
        if title.is_empty() {
            self.error = Some("タイトルを入れてください".into());
            cx.notify();
            return;
        }
        if body.trim().is_empty() {
            self.error = Some("本文を書いてください".into());
            cx.notify();
            return;
        }
        let client = self.session.client.clone();
        let visibility = self.visibility;
        let status = self.status;
        let kind = self.kind;
        let editing = self.editing.clone();
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Result<Written, ApiError> = runtime::run(async move {
                match editing {
                    None => {
                        client
                            .create_entry(&NewEntry {
                                kind,
                                title,
                                summary,
                                body,
                                tags,
                                visibility,
                                status,
                            })
                            .await
                    }
                    Some(old) => {
                        let content_changed = old.title != title || old.summary != summary || old.body != body;
                        let mut written = Written {
                            entry: old.clone(),
                            warnings: Vec::new(),
                        };
                        if content_changed {
                            written = client.revise(&old.id, &title, summary.as_deref(), &body).await?;
                        }
                        let new_status = status.filter(|s| *s != old.status);
                        let new_visibility = Some(visibility).filter(|v| *v != old.visibility);
                        let new_tags = Some(tags).filter(|t| *t != old.tags);
                        if new_status.is_some() || new_visibility.is_some() || new_tags.is_some() {
                            written.entry = client
                                .patch(&old.id, new_visibility, new_status, new_tags.as_deref())
                                .await?;
                        }
                        Ok(written)
                    }
                }
            })
            .await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(written) => {
                        cx.emit(Nav::Status("保存しました".into(), Tone::Success));
                        cx.emit(Nav::Saved {
                            id: written.entry.id,
                            warnings: written.warnings,
                        });
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for ComposePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let editing = self.edit_id.is_some();
        let loaded = !editing || self.editing.is_some();
        let kinds = EntryKind::ALL;
        let kind_index = kinds.iter().position(|k| *k == self.kind).unwrap_or(0);
        let sections: Vec<&str> = self.kind.recommended_sections().iter().map(|s| s.label).collect();
        let body_empty = self.body.read(cx).is_empty();

        let kind_picker = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .child(div().flex().child(if editing {
                Badge::new(kind_label(self.kind))
                    .icon(kind_icon(self.kind))
                    .into_any_element()
            } else {
                SegmentedControl::new(
                    "kind",
                    kinds
                        .iter()
                        .map(|k| Segment::label(kind_label(*k)).with_icon(kind_icon(*k))),
                    kind_index,
                )
                .on_select(cx.listener(move |this, index: &usize, _, cx| {
                    this.kind = kinds[*index];
                    cx.notify();
                }))
                .into_any_element()
            }))
            .child(
                Label::new(self.kind.description())
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );

        let statuses = [
            Status::Idea,
            Status::Investigating,
            Status::Preliminary,
            Status::Verified,
            Status::Superseded,
            Status::Retracted,
        ];
        let status = self.status.unwrap_or(self.kind.default_status());
        let vis_index = VISIBILITIES
            .iter()
            .position(|(v, _)| *v == self.visibility)
            .unwrap_or(0);

        let editor: gpui::AnyElement = if self.preview {
            Card::new()
                .p(theme.spacing.lg)
                .min_h(px(320.))
                .child(Markdown::new("preview", self.body.read(cx).text().to_string()))
                .into_any_element()
        } else {
            widgets::area(&self.body, "Markdown で書けます。Shift+Return で改行します", window, cx)
                .min_h(px(320.))
                .into_any_element()
        };

        let template_button = (body_empty && !sections.is_empty()).then(|| {
            Button::new("template", "推奨見出しを入れる")
                .size(ControlSize::Small)
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    let text = template(this.kind);
                    this.body.update(cx, |b, cx| b.set_text(text, cx));
                    cx.notify();
                }))
        });

        div().id("compose").size_full().overflow_y_scroll().child(
            div()
                .max_w(px(860.))
                .mx_auto()
                .p(theme.spacing.xl)
                .pb(px(64.))
                .flex()
                .flex_col()
                .gap(theme.spacing.lg)
                .child(
                    SectionHeader::new(if editing { "記事を編集" } else { "新規投稿" }).description(if editing {
                        "本文やタイトルを直すと新しい版になります。公開範囲・状態・タグは版を作らずに変わります。"
                    } else {
                        "見出しは推奨で、必須ではありません。短いメモでも気軽に残せます。"
                    }),
                )
                .when(!loaded, |el| {
                    el.child(atelier::skeleton_paragraph("compose-skeleton", 6, cx))
                })
                .when(loaded, |el| {
                    el.child(
                        FormGroup::new("種類")
                            .description(if editing {
                                "種類は投稿後には変えられません。"
                            } else {
                                "あとから変えられないので、内容に合うものを選んでください。"
                            })
                            .child(kind_picker),
                    )
                    .child(
                        FormGroup::untitled()
                            .child(FormField::new("タイトル").child(widgets::field(
                                &self.title,
                                "何についての記事か",
                                window,
                                cx,
                            )))
                            .child(
                                FormField::new("要約")
                                    .description("一覧と検索結果に出ます。空でもかまいません")
                                    .child(widgets::field(&self.summary, "ひとことで", window, cx)),
                            )
                            .child(
                                FormField::new("タグ")
                                    .description("空白かカンマで区切ります（例: rust sqlx）")
                                    .child(widgets::field(&self.tags, "", window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.spacing.sm)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.spacing.sm)
                                    .child(
                                        Tabs::new("body-tabs", ["本文", "プレビュー"], usize::from(self.preview))
                                            .on_select(cx.listener(|this, index: &usize, _, cx| {
                                                this.preview = *index == 1;
                                                cx.notify();
                                            })),
                                    )
                                    .child(div().flex_1())
                                    .children(template_button),
                            )
                            .when(!sections.is_empty(), |el| {
                                el.child(
                                    Label::new(format!("推奨見出し: {}", sections.join("、")))
                                        .size(TextSize::Small)
                                        .color(TextColor::Muted),
                                )
                            })
                            .child(editor),
                    )
                    .child(
                        FormGroup::new("公開と状態")
                            .child(
                                FormField::new("公開範囲")
                                    .description(visibility_help(self.visibility))
                                    .child(
                                        SegmentedControl::new(
                                            "visibility",
                                            VISIBILITIES.map(|(_, label)| Segment::label(label)),
                                            vis_index,
                                        )
                                        .on_select(cx.listener(
                                            |this, index: &usize, _, cx| {
                                                this.visibility = VISIBILITIES[*index].0;
                                                cx.notify();
                                            },
                                        )),
                                    ),
                            )
                            .child(
                                FormField::new("状態")
                                    .description("撤回は削除ではなく、記事を残したまま「撤回」と示します")
                                    .child(
                                        SegmentedControl::new(
                                            "status",
                                            statuses.map(|s| Segment::label(status_label(s))),
                                            statuses.iter().position(|s| *s == status).unwrap_or(0),
                                        )
                                        .size(ControlSize::Small)
                                        .on_select(cx.listener(
                                            move |this, index: &usize, _, cx| {
                                                this.status = Some(statuses[*index]);
                                                cx.notify();
                                            },
                                        )),
                                    ),
                            ),
                    )
                })
                .children(self.error.clone().map(|e| Label::new(e).color(TextColor::Danger)))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(theme.spacing.sm)
                        .child(
                            Button::new("cancel", "やめる")
                                .ghost()
                                .on_click(cx.listener(|_, _, window, cx| {
                                    window.dispatch_action(Box::new(crate::app::Back), cx);
                                })),
                        )
                        .child(
                            Button::new("save", if editing { "保存" } else { "投稿" })
                                .primary()
                                .icon(IconName::Check)
                                .disabled(self.saving || !loaded)
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_split_on_spaces_commas_and_japanese_commas() {
        assert_eq!(
            parse_tags("rust, sqlx  #gpui、日本語"),
            ["rust", "sqlx", "gpui", "日本語"]
        );
        assert!(parse_tags("  ").is_empty());
    }

    #[test]
    fn templates_list_the_recommended_sections() {
        assert_eq!(template(EntryKind::Memo), "");
        let t = template(EntryKind::Troubleshooting);
        assert!(t.starts_with("# 症状\n\n# 環境"), "{t}");
    }
}
