//! One entry: its body, metadata, versions, relations, provenance, comments and reproduction
//! reports.

use atelier::agent::CodeBlock;
use atelier::prelude::*;
use atelier::{
    Card, CopyButton, Divider, EmptyState, FormField, IconName, ListItem, Markdown, SectionHeader, Segment,
    SegmentedControl, SelectableText, Skeleton, Tabs, TextInput, Tone,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, SharedString, Window, div, px};
use knowledge_garden::model::{Comment, CommentKind, Entry, Links, ReproResult, VersionInfo};

use crate::api::{ApiError, NewComment};
use crate::app::{Nav, Session};
use crate::runtime;
use crate::widgets::{
    self, kind_icon, kind_label, local_time, posted_via_badge, relation_label, repro_label, repro_tone, status_label,
    status_tone, visibility_label,
};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Body,
    Comments,
    Versions,
    Links,
    Provenance,
}

const TABS: [Tab; 5] = [Tab::Body, Tab::Comments, Tab::Versions, Tab::Links, Tab::Provenance];

pub struct EntryPage {
    session: Session,
    id: String,
    /// A past version being read, or `None` for the current one.
    version: Option<i64>,
    entry: Option<Entry>,
    error: Option<ApiError>,
    versions: Vec<VersionInfo>,
    links: Option<Links>,
    comments: Vec<Comment>,
    tab: Tab,
    comment_kind: CommentKind,
    repro_result: ReproResult,
    comment: Entity<TextInput>,
    environment: Entity<TextInput>,
    posting: bool,
    comment_error: Option<String>,
    /// The server's advice on the version just written (missing sections, empty provenance).
    warnings: Vec<String>,
}

impl EventEmitter<Nav> for EntryPage {}

impl EntryPage {
    pub fn new(session: Session, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            session,
            id,
            version: None,
            entry: None,
            error: None,
            versions: Vec::new(),
            links: None,
            comments: Vec::new(),
            tab: Tab::Body,
            comment_kind: CommentKind::Comment,
            repro_result: ReproResult::Reproduced,
            comment: widgets::area_input("", cx),
            environment: widgets::input(cx),
            posting: false,
            comment_error: None,
            warnings: Vec::new(),
        };
        page.reload(window, cx);
        page
    }

    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The entry as shown, once loaded.
    pub fn entry(&self) -> Option<&Entry> {
        self.entry.as_ref()
    }

    pub fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let client = self.session.client.clone();
        let id = self.id.clone();
        let version = self.version;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (entry, versions, links, comments) = runtime::run(async move {
                let entry = client.entry(&id, version).await;
                let (versions, links, comments) =
                    tokio::join!(client.versions(&id), client.links(&id), client.comments(&id));
                (entry, versions, links, comments)
            })
            .await;
            this.update(cx, |this, cx| {
                match entry {
                    Ok(entry) => this.entry = Some(entry),
                    Err(e) => this.error = Some(e),
                }
                this.versions = versions.unwrap_or_default();
                this.links = links.ok();
                this.comments = comments.unwrap_or_default();
                cx.emit(Nav::EntryLoaded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_version(&mut self, version: Option<i64>, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.entry.as_ref().map(|e| e.current_version);
        self.version = version.filter(|v| Some(*v) != current);
        self.tab = Tab::Body;
        self.reload(window, cx);
    }

    fn can_edit(&self) -> bool {
        let Some(entry) = &self.entry else {
            return false;
        };
        entry.is_local
            && self.session.me.as_ref().is_some_and(|me| {
                me.user.username == entry.author.username || me.user.role == knowledge_garden::model::Role::Admin
            })
    }

    fn post_comment(&mut self, cx: &mut Context<Self>) {
        let body = widgets::text(&self.comment, cx);
        if body.is_empty() {
            self.comment_error = Some("本文を書いてください".into());
            cx.notify();
            return;
        }
        let environment = widgets::text(&self.environment, cx);
        let environment = match self.comment_kind {
            CommentKind::Reproduction if !environment.is_empty() => {
                Some(serde_json::from_str(&environment).unwrap_or_else(|_| serde_json::json!({ "note": environment })))
            }
            _ => None,
        };
        let comment = NewComment {
            kind: self.comment_kind,
            body,
            parent_id: None,
            result: (self.comment_kind == CommentKind::Reproduction).then_some(self.repro_result),
            environment,
        };
        let client = self.session.client.clone();
        let id = self.id.clone();
        self.posting = true;
        self.comment_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { client.comment(&id, &comment).await }).await;
            this.update(cx, |this, cx| {
                this.posting = false;
                match result {
                    Ok(comment) => {
                        this.comments.push(comment);
                        this.comment.update(cx, |c, cx| c.set_text("", cx));
                        this.environment.update(cx, |c, cx| c.set_text("", cx));
                        cx.emit(Nav::Status("コメントを投稿しました".into(), Tone::Success));
                    }
                    Err(e) => this.comment_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn header(&self, entry: &Entry, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let url = self.session.entry_url(&entry.id);
        let mut badges = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.xs)
            .child(Badge::new(kind_label(entry.kind)).icon(kind_icon(entry.kind)))
            .child(Badge::new(status_label(entry.status)).tone(status_tone(entry.status)))
            .child(Badge::new(visibility_label(entry.visibility)))
            .child(Badge::new(format!("第 {} 版", entry.version)));
        if let Some(badge) = posted_via_badge(entry.posted_via.kind, &entry.posted_via.name) {
            badges = badges.child(badge);
        }
        if !entry.is_local {
            badges = badges.child(Badge::new(format!("ミラー: {}", entry.origin)));
        }
        let r = &entry.reproductions;
        if r.reproduced + r.partial + r.failed > 0 {
            badges = badges.child(
                Badge::new(format!(
                    "再現 {} · 一部 {} · 失敗 {}",
                    r.reproduced, r.partial, r.failed
                ))
                .tone(if r.failed > 0 { Tone::Warning } else { Tone::Success }),
            );
        }
        let id = entry.id.clone();
        let edit = self.can_edit().then(|| {
            Button::new("edit", "編集").on_click(cx.listener(move |_, _, _, cx| cx.emit(Nav::EditEntry(id.clone()))))
        });
        let open_url = url.clone();
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.sm)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(theme.spacing.md)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(SelectableText::new("entry-title", entry.title.clone()).size(TextSize::XLarge)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.spacing.xs)
                            .children(edit)
                            .child(
                                Button::icon_only("open-browser", IconName::Eye)
                                    .ghost()
                                    .tooltip("ブラウザで開く")
                                    .on_click(move |_, _, cx| cx.open_url(&open_url)),
                            )
                            .child(CopyButton::new("copy-link", url.clone()).tooltip("記事の URL をコピー")),
                    ),
            )
            .children(entry.summary.clone().map(|s| Label::new(s).color(TextColor::Muted)))
            .child(badges)
            .child(
                Label::new(format!(
                    "{}（@{}）· {} 作成 · {} 更新",
                    entry.author.display_name,
                    entry.author.username,
                    local_time(&entry.created_at),
                    local_time(&entry.updated_at)
                ))
                .size(TextSize::Small)
                .color(TextColor::Faint),
            )
            .when(!entry.tags.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme.spacing.xs)
                        .children(entry.tags.iter().map(|t| {
                            Label::new(format!("#{t}"))
                                .size(TextSize::Small)
                                .color(TextColor::Accent)
                        })),
                )
            })
    }

    fn body(&self, entry: &Entry, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let session = self.session.clone();
        let entity = cx.entity().downgrade();
        let markdown = Markdown::new("entry-body", entry.body.clone()).on_link(move |url, _, cx| {
            match session.local_entry_id(url) {
                Some(id) => {
                    let id = id.to_string();
                    entity.update(cx, |_, cx| cx.emit(Nav::OpenEntry(id))).ok();
                }
                None => cx.open_url(url),
            }
        });
        let past = self.version.map(|v| {
            Card::new()
                .p(theme.spacing.md)
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .child(
                    Label::new(format!(
                        "第 {v} 版を表示しています（最新は第 {} 版）",
                        entry.current_version
                    ))
                    .color(TextColor::Warning),
                )
                .child(div().flex_1())
                .child(
                    Button::new("latest", "最新版を見る")
                        .size(ControlSize::Small)
                        .on_click(cx.listener(|this, _, window, cx| this.show_version(None, window, cx))),
                )
        });
        let advice = (!self.warnings.is_empty()).then(|| {
            Card::new()
                .p(theme.spacing.md)
                .flex()
                .flex_col()
                .gap(theme.spacing.xs)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .child(Label::new("投稿は受け付けられました。サーバーからの助言:").color(TextColor::Warning))
                        .child(div().flex_1())
                        .child(
                            Button::icon_only("dismiss-advice", IconName::Close)
                                .ghost()
                                .size(ControlSize::Small)
                                .tooltip("閉じる")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.warnings.clear();
                                    cx.notify();
                                })),
                        ),
                )
                .children(self.warnings.iter().map(|w| {
                    Label::new(format!("・{w}"))
                        .size(TextSize::Small)
                        .color(TextColor::Muted)
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.md)
            .children(advice)
            .children(past)
            .child(markdown)
            .into_any_element()
    }

    fn comments_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let mut column = div().flex().flex_col().gap(theme.spacing.md);
        if self.comments.is_empty() {
            column = column.child(
                EmptyState::new(IconName::Info, "まだコメントはありません")
                    .description("試してみた結果は「再現報告」として残すと、記事の信頼度の目安になります。"),
            );
        }
        for (i, c) in self.comments.iter().enumerate() {
            let mut head = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.spacing.xs)
                .child(Label::new(c.author.display_name.clone()).weight(gpui::FontWeight::SEMIBOLD))
                .child(
                    Label::new(local_time(&c.created_at))
                        .size(TextSize::Small)
                        .color(TextColor::Faint),
                );
            if let Some(badge) = posted_via_badge(c.posted_via.kind, &c.posted_via.name) {
                head = head.child(badge);
            }
            if let (CommentKind::Reproduction, Some(result)) = (c.kind, c.result) {
                head = head.child(Badge::new(repro_label(result)).tone(repro_tone(result)).dot());
            }
            if let Some(v) = c.entry_version {
                head = head.child(
                    Label::new(format!("第 {v} 版"))
                        .size(TextSize::Small)
                        .color(TextColor::Faint),
                );
            }
            let environment = c.environment.as_ref().filter(|v| !v.is_null()).map(|v| {
                CodeBlock::new(("comment-env", i), serde_json::to_string_pretty(v).unwrap_or_default())
                    .title("試した環境")
            });
            column = column.child(
                Card::new()
                    .p(theme.spacing.md)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.sm)
                    .when(c.parent_id.is_some(), |card| card.ml(theme.spacing.xl))
                    .child(head)
                    .child(Markdown::new(("comment", i), c.body.clone()))
                    .children(environment),
            );
        }
        if self.session.signed_in() && self.entry.is_some() {
            let kinds = [CommentKind::Comment, CommentKind::Reproduction];
            let results = [ReproResult::Reproduced, ReproResult::Partial, ReproResult::Failed];
            let reproduction = self.comment_kind == CommentKind::Reproduction;
            column = column.child(Divider::horizontal()).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.sm)
                    .child(SectionHeader::caption("書き込む"))
                    .child(
                        div().flex().child(
                            SegmentedControl::new(
                                "comment-kind",
                                [Segment::label("コメント"), Segment::label("再現報告")],
                                kinds.iter().position(|k| *k == self.comment_kind).unwrap_or(0),
                            )
                            .size(ControlSize::Small)
                            .on_select(cx.listener(move |this, index: &usize, _, cx| {
                                this.comment_kind = kinds[*index];
                                cx.notify();
                            })),
                        ),
                    )
                    .when(reproduction, |el| {
                        el.child(
                            FormField::new("結果").child(
                                SegmentedControl::new(
                                    "repro-result",
                                    results.map(|r| Segment::label(repro_label(r))),
                                    results.iter().position(|r| *r == self.repro_result).unwrap_or(0),
                                )
                                .on_select(cx.listener(
                                    move |this, index: &usize, _, cx| {
                                        this.repro_result = results[*index];
                                        cx.notify();
                                    },
                                )),
                            ),
                        )
                        .child(
                            FormField::new("試した環境")
                                .description(
                                    "JSON（例: {\"os\": \"macOS 26\", \"rustc\": \"1.97\"}）か、ひとことの説明",
                                )
                                .child(widgets::field(
                                    &self.environment,
                                    "OS、バージョン、ハードウェアなど",
                                    window,
                                    cx,
                                )),
                        )
                    })
                    .child(
                        FormField::new(if reproduction {
                            "やったこと・結果"
                        } else {
                            "本文"
                        })
                        .description("Markdown で書けます。Shift+Return で改行します")
                        .when_some(self.comment_error.clone(), FormField::error)
                        .child(widgets::area(&self.comment, "", window, cx).min_h(px(96.))),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new(
                                "post-comment",
                                if reproduction {
                                    "再現報告を投稿"
                                } else {
                                    "コメントを投稿"
                                },
                            )
                            .primary()
                            .disabled(self.posting)
                            .on_click(cx.listener(|this, _, _, cx| this.post_comment(cx))),
                        ),
                    ),
            );
        } else if !self.session.signed_in() {
            column = column.child(
                Label::new("ログインするとコメントや再現報告を書けます。")
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            );
        }
        column.into_any_element()
    }

    fn versions_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let shown = self.version.or(self.entry.as_ref().map(|e| e.current_version));
        let mut column = div().flex().flex_col().gap(px(2.)).child(
            Label::new("公開した版は書き換えられません。内容を直すと新しい版になります。")
                .size(TextSize::Small)
                .color(TextColor::Muted),
        );
        column = column.child(div().h(theme.spacing.xs));
        for v in self.versions.iter().rev() {
            let version = v.version;
            column = column.child(
                ListItem::new(
                    ("version", version as usize),
                    format!("第 {} 版 · {}", v.version, v.title),
                )
                .icon(IconName::History)
                .description(format!("{} · {}", local_time(&v.created_at), v.posted_via.name))
                .selected(shown == Some(version))
                .on_click(cx.listener(move |this, _, window, cx| this.show_version(Some(version), window, cx))),
            );
        }
        column.into_any_element()
    }

    fn links_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let Some(links) = &self.links else {
            return Skeleton::new("links-skeleton").h(px(40.)).into_any_element();
        };
        if links.outgoing.is_empty() && links.incoming.is_empty() {
            return EmptyState::new(IconName::File, "関係はまだありません")
                .description("引用、支持、反論、再現などの関係は API や MCP から付けられます。")
                .into_any_element();
        }
        let mut column = div().flex().flex_col().gap(px(2.));
        if !links.outgoing.is_empty() {
            column = column.child(SectionHeader::caption("この記事から"));
            for (i, l) in links.outgoing.iter().enumerate() {
                let target = l.target_id.clone();
                let uri = l.target_uri.clone();
                let mut item = ListItem::new(
                    ("out-link", i),
                    format!(
                        "{}: {}",
                        relation_label(l.relation),
                        l.target_title.clone().unwrap_or_else(|| l.target_uri.clone())
                    ),
                )
                .icon(IconName::ChevronRight)
                .on_click(cx.listener(move |_, _, _, cx| match &target {
                    Some(id) => cx.emit(Nav::OpenEntry(id.clone())),
                    None => cx.open_url(&uri),
                }));
                if let Some(note) = &l.note {
                    item = item.description(note.clone());
                }
                column = column.child(item);
            }
        }
        if !links.incoming.is_empty() {
            column = column
                .child(div().h(theme.spacing.sm))
                .child(SectionHeader::caption("この記事へ（被リンク）"));
            for (i, l) in links.incoming.iter().enumerate() {
                let source = l.source_id.clone();
                let mut item = ListItem::new(
                    ("in-link", i),
                    format!("{}: {}", relation_label(l.relation), l.source_title),
                )
                .icon(IconName::ChevronLeft)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(Nav::OpenEntry(source.clone()))));
                if let Some(note) = &l.note {
                    item = item.description(note.clone());
                }
                column = column.child(item);
            }
        }
        column.into_any_element()
    }

    fn provenance_tab(&self, entry: &Entry, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let empty = entry.provenance.as_object().is_none_or(|o| o.is_empty());
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.sm)
            .child(
                Label::new("来歴（provenance）: 使ったモデル、道具、コミット、実行したコマンド、環境など。")
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
            )
            .child(if empty {
                Label::new("記録されていません")
                    .color(TextColor::Faint)
                    .into_any_element()
            } else {
                CodeBlock::new(
                    "provenance",
                    serde_json::to_string_pretty(&entry.provenance).unwrap_or_default(),
                )
                .title("provenance")
                .copyable()
                .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for EntryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let content: gpui::AnyElement = match (&self.entry, &self.error) {
            (_, Some(error)) => EmptyState::new(IconName::Alert, "記事を開けませんでした")
                .description(error.to_string())
                .action(
                    Button::new("retry", "もう一度試す")
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .into_any_element(),
            (None, None) => div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .child(Skeleton::new("title-skeleton").h(px(32.)).w(px(420.)))
                .child(Skeleton::new("meta-skeleton").h(px(20.)).w(px(280.)))
                .child(atelier::skeleton_paragraph("body-skeleton", 8, cx))
                .into_any_element(),
            (Some(entry), None) => {
                let entry = entry.clone();
                let comment_count = self.comments.len();
                let labels: [SharedString; 5] = [
                    "本文".into(),
                    if comment_count > 0 {
                        format!("コメント {comment_count}").into()
                    } else {
                        "コメント".into()
                    },
                    format!("版の履歴 {}", self.versions.len().max(1)).into(),
                    "関係".into(),
                    "来歴".into(),
                ];
                let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
                let tab: gpui::AnyElement = match self.tab {
                    Tab::Body => self.body(&entry, cx),
                    Tab::Comments => self.comments_tab(window, cx),
                    Tab::Versions => self.versions_tab(cx),
                    Tab::Links => self.links_tab(cx),
                    Tab::Provenance => self.provenance_tab(&entry, cx),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.lg)
                    .child(self.header(&entry, cx))
                    .child(Tabs::new("entry-tabs", labels, selected).on_select(cx.listener(
                        |this, index: &usize, _, cx| {
                            this.tab = TABS[*index];
                            cx.notify();
                        },
                    )))
                    .child(tab)
                    .into_any_element()
            }
        };
        div().id("entry").size_full().overflow_y_scroll().child(
            div()
                .max_w(px(860.))
                .mx_auto()
                .p(theme.spacing.xl)
                .pb(px(64.))
                .child(content),
        )
    }
}
