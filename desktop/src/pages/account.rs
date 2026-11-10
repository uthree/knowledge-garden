//! Settings: how the app looks, who is signed in, agent API keys and invites.

use atelier::prelude::*;
use atelier::{
    Card, Checkbox, CopyButton, FormField, FormGroup, IconName, ListItem, SectionHeader, Segment, SegmentedControl,
    Switch, TextInput, Tone,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, Window, div, px};
use knowledge_garden::model::{Token, TokenKind};

use crate::api::ApiError;
use crate::app::{Nav, Session};
use crate::runtime;
use crate::settings::{AppearanceChoice, Settings};
use crate::update::{self, UpdateState, Updater};
use crate::widgets::{self, local_time};

/// A secret shown once, right after it was issued.
struct Shown {
    title: String,
    secret: String,
    /// What to do with it, e.g. an MCP command line.
    usage: Option<String>,
}

pub struct AccountPage {
    session: Option<Session>,
    settings: Settings,
    tokens: Vec<Token>,
    tokens_error: Option<ApiError>,
    token_name: Entity<TextInput>,
    allow_entries: bool,
    allow_comments: bool,
    shown: Option<Shown>,
    busy: bool,
    error: Option<String>,
    updater: Entity<Updater>,
    _observe_updater: gpui::Subscription,
}

impl EventEmitter<Nav> for AccountPage {}

const APPEARANCES: [(AppearanceChoice, &str); 3] = [
    (AppearanceChoice::System, "システムに合わせる"),
    (AppearanceChoice::Light, "ライト"),
    (AppearanceChoice::Dark, "ダーク"),
];

/// How a coding agent registers this Garden's MCP endpoint with a token.
pub fn mcp_command(base_url: &str, secret: &str) -> String {
    format!(
        "claude mcp add --transport http knowledge-garden {base_url}/mcp --header \"Authorization: Bearer {secret}\""
    )
}

impl AccountPage {
    pub fn new(
        session: Option<Session>,
        settings: Settings,
        updater: Entity<Updater>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            _observe_updater: cx.observe(&updater, |_, _, cx| cx.notify()),
            updater,
            session,
            settings,
            tokens: Vec::new(),
            tokens_error: None,
            token_name: widgets::input_with("claude-code", cx),
            allow_entries: true,
            allow_comments: true,
            shown: None,
            busy: false,
            error: None,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone().filter(Session::signed_in) else {
            return;
        };
        let client = session.client.clone();
        cx.spawn(async move |this, cx| {
            let tokens = runtime::run(async move { client.tokens().await }).await;
            this.update(cx, |this, cx| {
                match tokens {
                    Ok(t) => {
                        this.tokens = t;
                        this.tokens_error = None;
                    }
                    Err(e) => this.tokens_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_appearance(&mut self, choice: AppearanceChoice, cx: &mut Context<Self>) {
        self.settings.appearance = choice;
        cx.emit(Nav::SettingsChanged(self.settings.clone()));
        cx.notify();
    }

    fn toggle_auto_update(&mut self, cx: &mut Context<Self>) {
        self.settings.update.auto_check = !self.settings.update.auto_check;
        cx.emit(Nav::SettingsChanged(self.settings.clone()));
        cx.notify();
    }

    fn update_group(&self, cx: &mut Context<Self>) -> FormGroup {
        let theme = cx.theme().clone();
        let updater = self.updater.read(cx);
        let (status, color) = match updater.state() {
            UpdateState::Idle => ("まだ確認していません".to_string(), TextColor::Muted),
            UpdateState::Checking => ("確認しています…".into(), TextColor::Muted),
            UpdateState::UpToDate => ("最新の版です".into(), TextColor::Success),
            UpdateState::Manual(a) => (
                format!(
                    "バージョン {} が出ています。この版は自分では入れ替えられないので、リリースページから入れてください",
                    a.version
                ),
                TextColor::Accent,
            ),
            UpdateState::Downloading { available, received } => (
                match &available.asset {
                    Some(asset) if asset.size > 0 => format!(
                        "バージョン {} をダウンロードしています（{}%）",
                        available.version,
                        received * 100 / asset.size
                    ),
                    _ => format!("バージョン {} をダウンロードしています", available.version),
                },
                TextColor::Muted,
            ),
            UpdateState::Ready(a) => (
                format!("バージョン {} を入れました。再起動すると切り替わります", a.version),
                TextColor::Accent,
            ),
            UpdateState::Failed(e) => (format!("更新できませんでした: {e}"), TextColor::Danger),
        };
        let busy = matches!(updater.state(), UpdateState::Checking | UpdateState::Downloading { .. });
        let action = match updater.state() {
            UpdateState::Ready(_) => Button::new("relaunch-now", "再起動して更新")
                .primary()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Relaunch)))
                .into_any_element(),
            UpdateState::Manual(a) => {
                let page = a.page();
                Button::new("open-release", "リリースページを開く")
                    .icon(IconName::ArrowUp)
                    .on_click(move |_, _, cx| cx.open_url(&page))
                    .into_any_element()
            }
            _ => Button::new("check-update", "今すぐ確認")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.updater.update(cx, |u, cx| u.check(cx))))
                .into_any_element(),
        };
        let installable = updater.installable();
        FormGroup::new("アップデート")
            .description(if installable {
                "新しいリリースが出ると裏でダウンロードして入れ替え、再起動を案内します。"
            } else {
                "開発用のビルドや書き込めない場所に置いた版は自分では更新せず、リリースページを案内します。"
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.md)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(Label::new(format!("バージョン {}", update::current_version())))
                            .child(Label::new(status).size(TextSize::Small).color(color)),
                    )
                    .child(action),
            )
            .child(
                FormField::new("自動で確認する")
                    .description("起動の少しあとと、その後は数時間ごと。切り替えは次の起動から")
                    .child(
                        Switch::new("auto-update", self.settings.update.auto_check)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_auto_update(cx))),
                    ),
            )
    }

    fn set_direction(&mut self, direction: atelier::Direction, cx: &mut Context<Self>) {
        self.settings.direction = direction.id().to_string();
        cx.emit(Nav::SettingsChanged(self.settings.clone()));
        cx.notify();
    }

    fn issue_token(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let name = widgets::text(&self.token_name, cx);
        if name.is_empty() {
            self.error = Some("名前を入れてください（例: claude-code）".into());
            cx.notify();
            return;
        }
        let mut scopes = vec!["read"];
        if self.allow_entries {
            scopes.push("entries:write");
        }
        if self.allow_comments {
            scopes.push("comments:write");
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let client = session.client.clone();
        let base = client.base_url().to_string();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { client.create_agent_token(&name, &scopes).await }).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(issued) => {
                        this.shown = Some(Shown {
                            title: format!("エージェント用 API キー「{}」", issued.token.name),
                            usage: Some(mcp_command(&base, &issued.secret)),
                            secret: issued.secret,
                        });
                        this.refresh(cx);
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn revoke(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let client = session.client.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { client.revoke_token(&id).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => cx.emit(Nav::Status("キーを失効させました".into(), Tone::Success)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn invite(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let client = session.client.clone();
        let base = client.base_url().to_string();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move { client.create_invite("member", None).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(issued) => {
                        this.shown = Some(Shown {
                            title: "招待リンク（7日間、1回限り）".into(),
                            usage: None,
                            secret: format!("{base}/register?invite={}", issued.code),
                        })
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn token_row(&self, index: usize, token: &Token, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let scopes: Vec<&str> = token.scopes.iter().map(|s| s.as_str()).collect();
        let used = token
            .last_used_at
            .as_ref()
            .map(|t| format!("最後に使用 {}", local_time(t)))
            .unwrap_or_else(|| "未使用".into());
        let current = self
            .session
            .as_ref()
            .and_then(|s| s.me.as_ref())
            .is_some_and(|me| me.token.id == token.id);
        let id = token.id.clone();
        ListItem::new(("token", index), token.name.clone())
            .icon(if token.kind == TokenKind::Agent {
                IconName::Sparkles
            } else {
                IconName::User
            })
            .description(format!(
                "{} · {} · {}",
                if token.kind == TokenKind::Agent {
                    "エージェント"
                } else {
                    "人間"
                },
                scopes.join(", "),
                used
            ))
            .trailing(if current {
                Badge::new("このアプリ").tone(Tone::Accent).into_any_element()
            } else {
                Button::new(("revoke", index), "失効")
                    .size(ControlSize::Small)
                    .variant(atelier::ButtonVariant::Danger)
                    .on_click(cx.listener(move |this, _, _, cx| this.revoke(id.clone(), cx)))
                    .into_any_element()
            })
    }
}

impl Render for AccountPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let appearance_index = APPEARANCES
            .iter()
            .position(|(a, _)| *a == self.settings.appearance)
            .unwrap_or(0);
        let directions = atelier::Direction::ALL;
        let direction_index = directions
            .iter()
            .position(|d| d.id() == self.settings.direction)
            .unwrap_or(0);

        let mut page =
            div()
                .max_w(px(720.))
                .mx_auto()
                .p(theme.spacing.xl)
                .pb(px(64.))
                .flex()
                .flex_col()
                .gap(theme.spacing.lg)
                .child(SectionHeader::new("設定"))
                .child(
                    FormGroup::new("表示")
                        .child(
                            FormField::new("外観").child(
                                SegmentedControl::new(
                                    "appearance",
                                    APPEARANCES.map(|(_, label)| Segment::label(label)),
                                    appearance_index,
                                )
                                .on_select(cx.listener(
                                    |this, index: &usize, _, cx| this.set_appearance(APPEARANCES[*index].0, cx),
                                )),
                            ),
                        )
                        .child(
                            FormField::new("テーマ")
                                .description("Atelier の方向性。Glass が標準です")
                                .child(
                                    SegmentedControl::new(
                                        "direction",
                                        directions.map(|d| Segment::label(d.label())),
                                        direction_index,
                                    )
                                    .on_select(cx.listener(
                                        move |this, index: &usize, _, cx| this.set_direction(directions[*index], cx),
                                    )),
                                ),
                        ),
                );

        if let Some(shown) = &self.shown {
            page = page.child(
                Card::new()
                    .p(theme.spacing.lg)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.sm)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().child(Label::heading(shown.title.clone())))
                            .child(
                                Button::icon_only("hide-secret", IconName::Close)
                                    .ghost()
                                    .size(ControlSize::Small)
                                    .tooltip("閉じる")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.shown = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        Label::new("この画面を離れると二度と表示されません。いまコピーしてください。")
                            .size(TextSize::Small)
                            .color(TextColor::Warning),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.sm)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(atelier::SelectableText::new("shown-secret", shown.secret.clone()).mono()),
                            )
                            .child(CopyButton::new("copy-secret", shown.secret.clone())),
                    )
                    .children(shown.usage.clone().map(|usage| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.spacing.xs)
                            .child(
                                Label::new("Claude Code に登録する例（Codex などは docs/agent-guide.md を参照）")
                                    .size(TextSize::Small)
                                    .color(TextColor::Muted),
                            )
                            .child(
                                atelier::agent::CodeBlock::new("mcp-command", usage)
                                    .title("MCP")
                                    .copyable(),
                            )
                    })),
            );
        }

        match self.session.clone() {
            Some(session) if session.signed_in() => {
                let me = session.me.clone().expect("signed in");
                page = page.child(
                    FormGroup::new("アカウント").child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.md)
                            .child(Avatar::person(me.user.display_name.chars().take(2).collect::<String>()))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(Label::new(me.user.display_name.clone()))
                                    .child(
                                        Label::new(format!(
                                            "@{} · {} · {}",
                                            me.user.username,
                                            if session.is_admin() {
                                                "管理者"
                                            } else {
                                                "メンバー"
                                            },
                                            session.client.base_url()
                                        ))
                                        .size(TextSize::Small)
                                        .color(TextColor::Muted),
                                    ),
                            )
                            .child(
                                Button::new("sign-out", "ログアウト")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::SignedOut))),
                            ),
                    ),
                );
                let mut keys = div().flex().flex_col().gap(px(2.));
                for (i, token) in self.tokens.iter().filter(|t| t.revoked_at.is_none()).enumerate() {
                    keys = keys.child(self.token_row(i, token, cx));
                }
                page = page.child(
                    FormGroup::new("エージェント用 API キー")
                        .description(
                            "コーディングエージェントが MCP や API でこの Garden を使うためのキー。エージェントごとに分けると、どのエージェントが書いたか分かり、個別に失効できます。",
                        )
                        .children(self.tokens_error.clone().map(|e| Label::new(e.to_string()).color(TextColor::Danger)))
                        .child(keys)
                        .child(
                            FormField::new("新しいキーの名前")
                                .child(widgets::field(&self.token_name, "claude-code", window, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(theme.spacing.md)
                                .child(Checkbox::new("scope-read", true).label("読む").disabled(true))
                                .child(
                                    Checkbox::new("scope-entries", self.allow_entries)
                                        .label("記事を書く")
                                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                                            this.allow_entries = *on;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Checkbox::new("scope-comments", self.allow_comments)
                                        .label("コメント・再現報告を書く")
                                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                                            this.allow_comments = *on;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(
                            div().flex().justify_end().child(
                                Button::new("issue-token", "キーを発行")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.issue_token(cx))),
                            ),
                        ),
                );
                if session.is_admin() {
                    page = page.child(
                        FormGroup::new("招待")
                            .description("招待リンクを受け取った人が、ブラウザでユーザー登録できます。")
                            .child(
                                div().flex().justify_end().child(
                                    Button::new("invite", "招待リンクを作る")
                                        .icon(IconName::User)
                                        .on_click(cx.listener(|this, _, _, cx| this.invite(cx))),
                                ),
                            ),
                    );
                }
            }
            _ => {
                page = page.child(
                    FormGroup::new("アカウント").child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.md)
                            .child(
                                Label::new("ログインしていません。")
                                    .color(TextColor::Muted)
                                    .size(TextSize::Small),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("go-connect", "ログイン")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Connect))),
                            ),
                    ),
                );
            }
        }
        page = page.child(self.update_group(cx));
        page = page.children(self.error.clone().map(|e| Label::new(e).color(TextColor::Danger)));
        div().id("account").size_full().overflow_y_scroll().child(page)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mcp_command_names_the_endpoint_and_the_key() {
        assert_eq!(
            super::mcp_command("http://localhost:8787", "kg_x"),
            "claude mcp add --transport http knowledge-garden http://localhost:8787/mcp --header \"Authorization: Bearer kg_x\""
        );
    }
}
