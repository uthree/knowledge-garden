//! The entry list and the search: one page, because a search is the list narrowed by words.

use atelier::prelude::*;
use atelier::{
    EmptyState, IconName, ListItem, SectionHeader, Segment, SegmentedControl, Skeleton, TextInput, TextInputEvent, Tone,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, SharedString, Subscription, Window, div, px};
use knowledge_garden::model::{EntryKind, EntrySummary, TagCount};

use crate::api::{ApiError, Filter, SearchHit, SearchQuery};
use crate::app::{Nav, Session};
use crate::runtime;
use crate::widgets::{self, kind_icon, kind_label, posted_via_badge, status_label, status_tone};

const PAGE_SIZE: i64 = 50;

/// One row: an entry with the excerpt a search found, if any.
struct Row {
    entry: EntrySummary,
    snippet: Option<String>,
}

pub struct BrowsePage {
    session: Session,
    search: Entity<TextInput>,
    /// Whether the search box holds a regular expression rather than words.
    regex: bool,
    kind: Option<EntryKind>,
    tag: Option<String>,
    /// `None` for everything, `local` or `remote`.
    origin: Option<&'static str>,
    rows: Vec<Row>,
    total: i64,
    loading: bool,
    error: Option<ApiError>,
    tags: Vec<TagCount>,
    /// The query the rows answer, to drop answers to older ones that arrive late.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Nav> for BrowsePage {}

impl BrowsePage {
    pub fn new(session: Session, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = widgets::input(cx);
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, _, event: &TextInputEvent, _, cx| {
                if *event == TextInputEvent::Submit {
                    this.reload(cx);
                }
                if *event == TextInputEvent::Cancel {
                    this.search.update(cx, |s, cx| s.set_text("", cx));
                    this.reload(cx);
                }
            }),
        ];
        let mut page = Self {
            session,
            search,
            regex: false,
            kind: None,
            tag: None,
            origin: None,
            rows: Vec::new(),
            total: 0,
            loading: false,
            error: None,
            tags: Vec::new(),
            generation: 0,
            _subscriptions: subscriptions,
        };
        page.reload(cx);
        page.load_tags(cx);
        page
    }

    pub fn kind(&self) -> Option<EntryKind> {
        self.kind
    }

    pub fn set_kind(&mut self, kind: Option<EntryKind>, _: &mut Window, cx: &mut Context<Self>) {
        self.kind = kind;
        self.reload(cx);
    }

    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| {
            s.select_all(cx);
            s.focus(window);
        });
    }

    fn filter(&self, offset: i64) -> Filter {
        Filter {
            kind: self.kind,
            tag: self.tag.clone(),
            origin: self.origin.map(str::to_string),
            limit: Some(PAGE_SIZE),
            offset: (offset > 0).then_some(offset),
            ..Default::default()
        }
    }

    /// Loads the first page again with the current words and filters.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.load(0, cx);
    }

    fn load(&mut self, offset: i64, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let words = widgets::text(&self.search, cx);
        let filter = self.filter(offset);
        let regex = self.regex;
        let client = self.session.client.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move {
                if words.is_empty() {
                    client.entries(&filter).await.map(|page| {
                        let rows = page
                            .items
                            .into_iter()
                            .map(|entry| Row { entry, snippet: None })
                            .collect();
                        (rows, page.total)
                    })
                } else {
                    let query = SearchQuery {
                        q: (!regex).then(|| words.clone()),
                        regex: regex.then_some(words),
                        filter,
                    };
                    client.search(&query).await.map(|page| {
                        let rows = page
                            .items
                            .into_iter()
                            .map(|SearchHit { entry, snippet, .. }| Row { entry, snippet })
                            .collect();
                        (rows, page.total)
                    })
                }
            })
            .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((rows, total)) => {
                        if offset == 0 {
                            this.rows = rows;
                        } else {
                            this.rows.extend(rows);
                        }
                        this.total = total;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_tags(&mut self, cx: &mut Context<Self>) {
        let client = self.session.client.clone();
        cx.spawn(async move |this, cx| {
            let tags = runtime::run(async move { client.tags().await }).await;
            this.update(cx, |this, cx| {
                if let Ok(mut tags) = tags {
                    tags.truncate(24);
                    this.tags = tags;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let e = &row.entry;
        let description = row
            .snippet
            .clone()
            .or_else(|| e.summary.clone())
            .unwrap_or_else(|| format!("{} · {}", kind_label(e.kind), e.author.display_name));
        let id = e.id.clone();
        let theme = cx.theme().clone();
        let mut trailing = div().flex().items_center().gap(theme.spacing.xs);
        if let Some(badge) = posted_via_badge(e.posted_via.kind, &e.posted_via.name) {
            trailing = trailing.child(badge);
        }
        if !e.is_local {
            trailing = trailing.child(Badge::new("ミラー").tone(Tone::Neutral));
        }
        let repro = e.reproductions.reproduced + e.reproductions.partial + e.reproductions.failed;
        if repro > 0 {
            trailing = trailing.child(Badge::new(format!("再現 {}/{repro}", e.reproductions.reproduced)).tone(
                if e.reproductions.failed > 0 {
                    Tone::Warning
                } else {
                    Tone::Success
                },
            ));
        }
        trailing = trailing
            .child(Badge::new(status_label(e.status)).tone(status_tone(e.status)))
            .child(
                Label::new(e.updated_at.with_timezone(&chrono::Local).format("%m/%d").to_string())
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint),
            );
        ListItem::new(("entry", index), e.title.clone())
            .icon(kind_icon(e.kind))
            .description(description)
            .description_lines(2)
            .trailing(trailing)
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(Nav::OpenEntry(id.clone()))))
    }
}

impl Render for BrowsePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let searching = !self.search.read(cx).is_empty();
        let title: SharedString = match self.kind {
            Some(kind) => kind_label(kind).into(),
            None => "記事".into(),
        };
        let count = if self.loading && self.rows.is_empty() {
            String::new()
        } else if searching {
            format!("{} 件見つかりました", self.total)
        } else {
            format!("{} 件", self.total)
        };
        let origin_index = match self.origin {
            None => 0,
            Some("local") => 1,
            _ => 2,
        };
        let placeholder = if self.regex {
            "正規表現（例: error\\[E\\d{4}\\]）"
        } else {
            "検索（語はすべて一致、\"フレーズ\"、-除外）"
        };

        let mut tags = div().flex().flex_wrap().gap(theme.spacing.xs);
        for (i, tag) in self.tags.iter().enumerate() {
            let selected = self.tag.as_deref() == Some(tag.tag.as_str());
            let name = tag.tag.clone();
            tags = tags.child(
                Button::new(("tag", i), format!("#{} {}", tag.tag, tag.count))
                    .size(ControlSize::Small)
                    .ghost()
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tag = if this.tag.as_deref() == Some(name.as_str()) {
                            None
                        } else {
                            Some(name.clone())
                        };
                        this.reload(cx);
                    })),
            );
        }

        let list: gpui::AnyElement = if let Some(error) = &self.error {
            EmptyState::new(IconName::Alert, "読み込めませんでした")
                .description(error.to_string())
                .action(Button::new("retry", "もう一度試す").on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
                .into_any_element()
        } else if self.loading && self.rows.is_empty() {
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.sm)
                .children((0..6usize).map(|i| Skeleton::new(("row-skeleton", i)).h(px(44.))))
                .into_any_element()
        } else if self.rows.is_empty() {
            let (title, description) = if searching {
                ("見つかりませんでした", "語を減らすか、絞り込みを外してみてください。")
            } else {
                ("まだ記事がありません", "「新規投稿」から最初の記事を書けます。")
            };
            EmptyState::new(if searching { IconName::Search } else { IconName::File }, title)
                .description(description)
                .into_any_element()
        } else {
            let mut column = div().flex().flex_col().gap(px(2.));
            for (i, row) in self.rows.iter().enumerate() {
                column = column.child(self.row(i, row, cx));
            }
            if (self.rows.len() as i64) < self.total {
                column = column.child(
                    div().pt(theme.spacing.sm).flex().justify_center().child(
                        Button::new("more", "さらに読み込む")
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let offset = this.rows.len() as i64;
                                this.load(offset, cx);
                            })),
                    ),
                );
            }
            column.into_any_element()
        };

        div().id("browse").size_full().overflow_y_scroll().child(
            div()
                .max_w(px(920.))
                .mx_auto()
                .p(theme.spacing.xl)
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .child(
                    SectionHeader::new(title).description(count).action(
                        Button::new("new-entry", "新規投稿")
                            .primary()
                            .icon(IconName::Plus)
                            .disabled(!self.session.signed_in())
                            .shortcut_for(&crate::app::NewEntry)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::NewEntry))),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .child(
                            div().flex_1().child(
                                widgets::field(&self.search, placeholder, window, cx)
                                    .icon(IconName::Search)
                                    .trailing(
                                        Button::new("search-go", "検索")
                                            .size(ControlSize::Small)
                                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                                    ),
                            ),
                        )
                        .child(
                            SegmentedControl::new(
                                "search-mode",
                                [Segment::label("語"), Segment::label("正規表現")],
                                usize::from(self.regex),
                            )
                            .size(ControlSize::Small)
                            .on_select(cx.listener(|this, index: &usize, _, cx| {
                                this.regex = *index == 1;
                                if !this.search.read(cx).is_empty() {
                                    this.reload(cx);
                                }
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .child(Label::caption("出所"))
                        .child(
                            SegmentedControl::new(
                                "origin",
                                [
                                    Segment::label("すべて"),
                                    Segment::label("この Garden"),
                                    Segment::label("連合先"),
                                ],
                                origin_index,
                            )
                            .size(ControlSize::Small)
                            .on_select(cx.listener(|this, index: &usize, _, cx| {
                                this.origin = match index {
                                    1 => Some("local"),
                                    2 => Some("remote"),
                                    _ => None,
                                };
                                this.reload(cx);
                            })),
                        ),
                )
                .when(!self.tags.is_empty(), |page| page.child(tags))
                .child(list),
        )
    }
}
