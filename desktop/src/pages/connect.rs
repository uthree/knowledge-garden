//! Choosing a Garden and signing in: the saved connections, and a form to add one.

use atelier::prelude::*;
use atelier::{
    EmptyState, FormField, FormGroup, IconName, ListItem, SectionHeader, Segment, SegmentedControl, TextInput,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, Subscription, Window, div, px};

use crate::api::{self, Client};
use crate::app::Nav;
use crate::runtime;
use crate::settings::{Connection, ConnectionKind, Settings, SettingsStore, new_id};
use crate::widgets::{self, PasswordField, PasswordSubmit};

#[derive(Clone, Copy, PartialEq)]
enum SignIn {
    Password,
    Token,
    Guest,
}

pub struct ConnectPage {
    store: SettingsStore,
    settings: Settings,
    url: Entity<TextInput>,
    username: Entity<TextInput>,
    password: Entity<PasswordField>,
    token: Entity<TextInput>,
    sign_in: SignIn,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Nav> for ConnectPage {}

impl ConnectPage {
    pub fn new(store: SettingsStore, settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(PasswordField::new);
        let subscriptions =
            vec![cx.subscribe_in(&password, window, |this, _, _: &PasswordSubmit, _, cx| this.connect(cx))];
        Self {
            store,
            settings,
            url: widgets::input(cx),
            username: widgets::input(cx),
            password,
            token: widgets::input(cx),
            sign_in: SignIn::Password,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_settings(&mut self, settings: Settings, cx: &mut Context<Self>) {
        self.settings = settings;
        cx.notify();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let base_url = match api::normalize_base_url(self.url.read(cx).text()) {
            Ok(url) => url,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let sign_in = self.sign_in;
        let username = widgets::text(&self.username, cx).to_lowercase();
        let password = self.password.read(cx).secret().to_string();
        let token = widgets::text(&self.token, cx);
        match sign_in {
            SignIn::Password if username.is_empty() || password.is_empty() => {
                self.error = Some("ユーザー名とパスワードを入れてください".into());
                cx.notify();
                return;
            }
            SignIn::Token if token.is_empty() => {
                self.error = Some("トークン（kg_ で始まる文字列）を貼り付けてください".into());
                cx.notify();
                return;
            }
            _ => {}
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(async move {
                let guest = Client::new(&base_url, None);
                let garden = guest.garden().await?;
                let (secret, username) = match sign_in {
                    SignIn::Password => {
                        let issued = guest.login(&username, &password).await?;
                        (Some(issued.secret), Some(issued.user.username))
                    }
                    SignIn::Token => {
                        let me = guest.with_token(Some(token.clone())).me().await?;
                        (Some(token), Some(me.user.username))
                    }
                    SignIn::Guest => (None, None),
                };
                Ok::<_, api::ApiError>((garden, secret, username, base_url))
            })
            .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((garden, secret, username, base_url)) => {
                        let existing = this.settings.connections.iter().find(|c| c.base_url == base_url);
                        let connection = Connection {
                            id: existing.map(|c| c.id.clone()).unwrap_or_else(new_id),
                            name: garden.name.clone(),
                            base_url,
                            kind: ConnectionKind::Remote,
                            username,
                        };
                        let mut settings = store.load();
                        settings.upsert(connection.clone());
                        let saved = store
                            .save(&settings)
                            .and_then(|_| store.set_token(&connection.id, secret.as_deref()));
                        if let Err(e) = saved {
                            this.error = Some(format!("接続先を保存できませんでした: {e}"));
                        } else {
                            this.password.update(cx, |p, cx| p.clear(cx));
                            this.token.update(cx, |t, cx| t.set_text("", cx));
                            this.settings = settings;
                            cx.emit(Nav::Connected(connection));
                        }
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn forget(&mut self, id: &str, cx: &mut Context<Self>) {
        let mut settings = self.store.load();
        settings.remove(id);
        let _ = self.store.set_token(id, None);
        if self.store.save(&settings).is_ok() {
            self.settings = settings.clone();
            cx.emit(Nav::SettingsChanged(settings));
        }
        cx.notify();
    }
}

impl Render for ConnectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.settings.current.clone();
        let mut saved = div().flex().flex_col().gap(px(2.));
        for (i, c) in self.settings.connections.iter().enumerate() {
            let connection = c.clone();
            let id = c.id.clone();
            let who = c
                .username
                .clone()
                .map(|u| format!(" · {u}"))
                .unwrap_or_else(|| " · ゲスト".into());
            saved = saved.child(
                ListItem::new(("saved", i), c.name.clone())
                    .icon(match c.kind {
                        ConnectionKind::Local => IconName::Folder,
                        ConnectionKind::Remote => IconName::Eye,
                    })
                    .description(format!("{}{who}", c.base_url))
                    .selected(current.as_deref() == Some(c.id.as_str()))
                    .trailing(
                        Button::icon_only(("forget", i), IconName::Trash)
                            .ghost()
                            .size(ControlSize::Small)
                            .tooltip("この接続先を削除（Garden の記事は消えません）")
                            .on_click(cx.listener(move |this, _, _, cx| this.forget(&id, cx))),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(Nav::Connected(connection.clone())))),
            );
        }
        let modes = [SignIn::Password, SignIn::Token, SignIn::Guest];
        let mode_index = modes.iter().position(|m| *m == self.sign_in).unwrap_or(0);
        let credentials: gpui::AnyElement = match self.sign_in {
            SignIn::Password => div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .child(FormField::new("ユーザー名").child(widgets::field(&self.username, "", window, cx)))
                .child(
                    FormField::new("パスワード")
                        .description("Web 版と同じパスワードです。このアプリ用のトークン（desktop）が発行されます")
                        .child(self.password.clone()),
                )
                .into_any_element(),
            SignIn::Token => FormField::new("トークン")
                .description("Web 版の設定や CLI（create-admin）で発行した人間用トークン")
                .child(widgets::field(&self.token, "kg_…", window, cx))
                .into_any_element(),
            SignIn::Guest => Label::new("ログインせずに公開記事を読みます（Garden がゲスト閲覧を許している場合）。")
                .size(TextSize::Small)
                .color(TextColor::Muted)
                .into_any_element(),
        };
        div().id("connect").size_full().overflow_y_scroll().child(
            div()
                .max_w(px(640.))
                .mx_auto()
                .p(theme.spacing.xl)
                .flex()
                .flex_col()
                .gap(theme.spacing.lg)
                .child(
                    SectionHeader::new("接続先").description("自宅の NAS やサーバーで動いている Garden に接続します。"),
                )
                .child(if self.settings.connections.is_empty() {
                    EmptyState::new(IconName::Folder, "まだ接続先がありません")
                        .description("下のフォームで Garden の URL を入れてください。")
                        .into_any_element()
                } else {
                    saved.into_any_element()
                })
                .child(
                    FormGroup::new("Garden を追加")
                        .child(
                            FormField::new("URL")
                                .description("例: http://nas.local:8080 や https://garden.example")
                                .child(widgets::field(&self.url, "http://nas.local:8080", window, cx)),
                        )
                        .child(
                            FormField::new("ログイン").child(
                                SegmentedControl::new(
                                    "sign-in",
                                    [
                                        Segment::label("パスワード"),
                                        Segment::label("トークン"),
                                        Segment::label("ゲスト"),
                                    ],
                                    mode_index,
                                )
                                .on_select(cx.listener(
                                    move |this, index: &usize, _, cx| {
                                        this.sign_in = modes[*index];
                                        this.error = None;
                                        cx.notify();
                                    },
                                )),
                            ),
                        )
                        .child(credentials),
                )
                .children(self.error.clone().map(|e| Label::new(e).color(TextColor::Danger)))
                .child(
                    div().flex().justify_end().child(
                        Button::new(
                            "connect",
                            if self.busy {
                                "接続しています…"
                            } else {
                                "接続"
                            },
                        )
                        .primary()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    ),
                )
                .child(
                    Label::new("トークンはこのコンピューターの設定フォルダーに、自分だけが読める形で保存されます。")
                        .size(TextSize::Small)
                        .color(TextColor::Faint),
                ),
        )
    }
}
