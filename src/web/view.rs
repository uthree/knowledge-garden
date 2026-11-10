//! Page chrome and small pieces of markup shared by the pages. The look comes
//! from the vendored Atelier web kit; `assets/garden.css` only lays it out.

use chrono::{DateTime, Utc};
use maud::{DOCTYPE, Markup, html};

use crate::model::*;
use crate::store::Session;

pub const ASSET_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn kind_label(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Memo => "メモ",
        EntryKind::ResearchNote => "研究ノート",
        EntryKind::ResearchReport => "研究レポート",
        EntryKind::Troubleshooting => "トラブルシューティング",
    }
}

pub fn kind_icon(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Memo => "bolt",
        EntryKind::ResearchNote => "brain",
        EntryKind::ResearchReport => "file",
        EntryKind::Troubleshooting => "wrench",
    }
}

pub fn status_label(s: Status) -> &'static str {
    match s {
        Status::Idea => "アイデア",
        Status::Investigating => "調査中",
        Status::Preliminary => "暫定",
        Status::Verified => "検証済み",
        Status::Superseded => "置き換え済み",
        Status::Retracted => "撤回",
    }
}

fn status_tone(s: Status) -> &'static str {
    match s {
        Status::Verified => "at-badge--success",
        Status::Retracted => "at-badge--danger",
        Status::Superseded => "at-badge--warning",
        _ => "",
    }
}

pub fn visibility_label(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "公開",
        Visibility::Unlisted => "限定公開（リンクを知っている人）",
        Visibility::Federated => "連合（メンバーと承認済みの Garden）",
        Visibility::Members => "メンバー限定",
        Visibility::Private => "非公開（自分だけ）",
    }
}

pub fn relation_label(r: Relation) -> &'static str {
    match r {
        Relation::Cites => "引用",
        Relation::Uses => "利用",
        Relation::Supports => "支持",
        Relation::Contradicts => "反論",
        Relation::Extends => "発展",
        Relation::Reproduces => "再現",
        Relation::Revises => "改訂",
        Relation::Supersedes => "置き換え",
    }
}

pub fn repro_label(r: ReproResult) -> &'static str {
    match r {
        ReproResult::Reproduced => "再現できた",
        ReproResult::Partial => "一部再現",
        ReproResult::Failed => "再現できなかった",
    }
}

fn repro_tone(r: ReproResult) -> &'static str {
    match r {
        ReproResult::Reproduced => "at-badge--success",
        ReproResult::Partial => "at-badge--warning",
        ReproResult::Failed => "at-badge--danger",
    }
}

/// A time, shown in UTC and turned into the reader's local time by `garden.js`.
pub fn time(t: &DateTime<Utc>) -> Markup {
    html! {
        time datetime=(t.to_rfc3339()) { (t.format("%Y-%m-%d %H:%M UTC")) }
    }
}

pub fn status_badge(s: Status) -> Markup {
    html! { span class={"at-badge " (status_tone(s))} { (status_label(s)) } }
}

pub fn repro_badge(r: ReproResult) -> Markup {
    html! { span class={"at-badge " (repro_tone(r))} { (repro_label(r)) } }
}

/// Who wrote something, and whether an agent posted it.
pub fn byline(author: &AuthorRef, via: &PostedVia) -> Markup {
    html! {
        span.kg-byline {
            a href={"/?author=" (urlencode(&author.username))} { (author.display_name) }
            @if via.kind == TokenKind::Agent {
                span.at-badge.at-badge--agent data-tooltip="エージェントが投稿" {
                    span data-icon="sparkles" {} (via.name)
                }
            }
        }
    }
}

pub fn repro_stats(r: &ReproStats) -> Markup {
    html! {
        @if r.reproduced > 0 { span.at-badge.at-badge--success { "再現 " (r.reproduced) } }
        @if r.partial > 0 { span.at-badge.at-badge--warning { "一部 " (r.partial) } }
        @if r.failed > 0 { span.at-badge.at-badge--danger { "失敗 " (r.failed) } }
    }
}

pub fn tag_links(tags: &[String]) -> Markup {
    html! {
        @for t in tags {
            a.kg-tag href={"/?tag=" (urlencode(t))} { "#" (t) }
        }
    }
}

pub fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// The host of a Garden's base URL, for showing where a mirrored entry came from.
pub fn host_of(base_url: &str) -> String {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| u.port().map_or(h.to_string(), |p| format!("{h}:{p}")))
        })
        .unwrap_or_else(|| base_url.to_string())
}

pub fn notice_error(message: &str) -> Markup {
    html! {
        div.kg-alert.kg-alert--error role="alert" {
            span data-icon="alert" {} span { (message) }
        }
    }
}

pub fn notice_info(message: &str) -> Markup {
    html! {
        div.kg-alert role="status" {
            span data-icon="info" {} span { (message) }
        }
    }
}

pub fn csrf_field(session: Option<&Session>) -> Markup {
    html! {
        @if let Some(s) = session {
            input type="hidden" name="csrf" value=(s.csrf);
        }
    }
}

/// What the sidebar marks as the current place.
#[derive(Default)]
pub struct Current {
    pub kind: Option<EntryKind>,
    pub tag: Option<String>,
    pub origin: Option<String>,
    pub page: Option<&'static str>,
}

pub struct Chrome<'a> {
    pub garden: &'a str,
    pub session: Option<&'a Session>,
    pub query: &'a str,
    pub tags: &'a [TagCount],
    pub current: &'a Current,
}

fn nav_item(href: &str, icon: &str, label: &str, current: bool) -> Markup {
    html! {
        a.at-list-item href=(href) aria-current=[current.then_some("page")] {
            span data-icon=(icon) {}
            span.at-list-item__label { (label) }
        }
    }
}

pub fn layout(chrome: &Chrome, title: &str, body: Markup) -> Markup {
    let c = chrome.current;
    let home = c.kind.is_none() && c.tag.is_none() && c.origin.is_none() && c.page == Some("home");
    let user = chrome.session.map(|s| &s.principal.user);
    html! {
        (DOCTYPE)
        html lang="ja" data-theme="glass-light" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                link rel="icon" href={"/assets/app-icon.svg?v=" (ASSET_VERSION)} type="image/svg+xml";
                link rel="icon" href={"/assets/app-icon.ico?v=" (ASSET_VERSION)} sizes="16x16 32x32 48x48";
                link rel="apple-touch-icon" href={"/assets/app-icon.png?v=" (ASSET_VERSION)};
                title { (title) " · " (chrome.garden) }
                script src={"/assets/theme.js?v=" (ASSET_VERSION)} {}
                link rel="stylesheet" href={"/assets/atelier/atelier.css?v=" (ASSET_VERSION)};
                link rel="stylesheet" href={"/assets/garden.css?v=" (ASSET_VERSION)};
                script type="module" src={"/assets/garden.js?v=" (ASSET_VERSION)} {}
            }
            body.at-root {
                div.at-window {
                    header.at-title-bar.at-chrome {
                        div.at-title-bar__side {
                            button.at-button.at-button--ghost.at-button--sm.at-button--icon.kg-nav-toggle type="button"
                                aria-label="メニュー" aria-controls="kg-nav" aria-expanded="false" {
                                span data-icon="menu" {}
                            }
                            a.kg-brand href="/" {
                                img.kg-brand__logo src={"/assets/app-icon.svg?v=" (ASSET_VERSION)} alt="" width="24" height="24";
                                span.kg-brand__name { (chrome.garden) }
                            }
                        }
                        div.at-title-bar__title { (title) }
                        div.at-title-bar__side.at-title-bar__side--end {
                            form.kg-search action="/" method="get" role="search" {
                                label.at-field {
                                    span data-icon="search" {}
                                    input type="search" name="q" value=(chrome.query) placeholder="記事を検索" aria-label="記事を検索";
                                }
                            }
                            @if user.is_some() {
                                a.at-button.at-button--primary.at-button--sm href="/new" {
                                    span data-icon="plus" {} "投稿"
                                }
                            } @else {
                                a.at-button.at-button--sm href="/login" { "ログイン" }
                            }
                        }
                    }
                    div.at-window__body {
                        nav.at-sidebar.at-chrome.kg-sidebar #kg-nav aria-label="ナビゲーション" {
                            div.at-list {
                                (nav_item("/", "file", "すべての記事", home))
                                @for k in EntryKind::ALL {
                                    (nav_item(&format!("/?kind={}", k.as_str()), kind_icon(*k), kind_label(*k), c.kind == Some(*k)))
                                }
                            }
                            div.at-list {
                                div.at-caption { "出所" }
                                (nav_item("/?origin=local", "folder", "この Garden", c.origin.as_deref() == Some("local")))
                                (nav_item("/?origin=remote", "refresh", "ほかの Garden", c.origin.as_deref() == Some("remote")))
                            }
                            @if !chrome.tags.is_empty() {
                                div.at-list {
                                    div.at-caption { "タグ" }
                                    @for t in chrome.tags {
                                        a.at-list-item href={"/?tag=" (urlencode(&t.tag))}
                                            aria-current=[(c.tag.as_deref() == Some(t.tag.as_str())).then_some("page")] {
                                            span.at-list-item__label { "#" (t.tag) }
                                            span.at-list-item__trailing.at-small.at-faint { (t.count) }
                                        }
                                    }
                                }
                            }
                            div.kg-sidebar__footer {
                                @if let Some(u) = user {
                                    span.at-avatar { (initials(&u.display_name)) }
                                    div.kg-sidebar__who {
                                        div { (u.display_name) }
                                        div.at-small.at-faint { "@" (u.username) }
                                    }
                                    a.at-button.at-button--ghost.at-button--sm.at-button--icon href="/settings"
                                        data-tooltip="設定" aria-label="設定" aria-current=[(c.page == Some("settings")).then_some("page")] {
                                        span data-icon="settings" {}
                                    }
                                } @else {
                                    span.at-small.at-faint { "閲覧中（ゲスト）" }
                                }
                            }
                        }
                        main.at-content {
                            div.kg-page { (body) }
                        }
                    }
                }
            }
        }
    }
}

fn initials(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// A bare page for errors that happen before the chrome can be built.
pub fn bare(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="ja" data-theme="glass-light" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="icon" href={"/assets/app-icon.svg?v=" (ASSET_VERSION)} type="image/svg+xml";
                link rel="icon" href={"/assets/app-icon.ico?v=" (ASSET_VERSION)} sizes="16x16 32x32 48x48";
                link rel="apple-touch-icon" href={"/assets/app-icon.png?v=" (ASSET_VERSION)};
                script src={"/assets/theme.js?v=" (ASSET_VERSION)} {}
                link rel="stylesheet" href={"/assets/atelier/atelier.css?v=" (ASSET_VERSION)};
                link rel="stylesheet" href={"/assets/garden.css?v=" (ASSET_VERSION)};
            }
            body.at-root.kg-bare { div.kg-page { (body) } }
        }
    }
}
