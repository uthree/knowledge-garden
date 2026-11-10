//! The Garden on this computer: start and stop it, how it listens, its log, backups and a
//! password for the browser.

use atelier::prelude::*;
use atelier::{
    Card, CopyButton, FormField, FormGroup, IconName, LogLevel, LogLine, LogView, SectionHeader, Switch, TextInput,
    Tone,
};
use gpui::prelude::*;
use gpui::{Context, Entity, EventEmitter, ScrollHandle, Subscription, Window, div, px};

use crate::app::{Nav, ServerAction};
use crate::local_server::{LocalServer, ServerStatus};
use crate::logs::LogRecord;
use crate::settings::LocalServerSettings;
use crate::widgets::{self, PasswordField, PasswordSubmit};

/// How many of the latest records the page draws.
const SHOWN_RECORDS: usize = 300;

pub struct ServerPage {
    server: Entity<LocalServer>,
    local: LocalServerSettings,
    /// The admin the app signs in to the local Garden as, once it has been set up.
    username: Option<String>,
    port: Entity<TextInput>,
    base_url: Entity<TextInput>,
    name: Entity<TextInput>,
    password: Entity<PasswordField>,
    log_scroll: ScrollHandle,
    last_seq: u64,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Nav> for ServerPage {}

impl ServerPage {
    pub fn new(
        server: Entity<LocalServer>,
        local: LocalServerSettings,
        username: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password = cx.new(PasswordField::new);
        let subscriptions = vec![
            cx.observe(&server, |this, _, cx| {
                this.follow_log(cx);
                cx.notify();
            }),
            cx.subscribe_in(&password, window, |this, _, _: &PasswordSubmit, _, cx| {
                this.set_password(cx)
            }),
        ];
        Self {
            port: widgets::input_with(&local.port.to_string(), cx),
            base_url: widgets::input_with(local.base_url.as_deref().unwrap_or(""), cx),
            name: widgets::input_with(&local.name, cx),
            server,
            local,
            username,
            password,
            log_scroll: ScrollHandle::new(),
            last_seq: 0,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    /// Takes settings and the signed-in admin as they are now (after the first start, say).
    pub fn update_from(&mut self, local: LocalServerSettings, username: Option<String>, cx: &mut Context<Self>) {
        if self.local.base_url.is_none()
            && let Some(url) = local.base_url.clone()
        {
            self.base_url.update(cx, |input, cx| input.set_text(url, cx));
        }
        self.local = local;
        self.username = username;
        cx.notify();
    }

    /// Keeps the log scrolled to the newest record while it was already at the bottom.
    fn follow_log(&mut self, cx: &mut Context<Self>) {
        let newest = self.server.read(cx).logs().records().last().map(|r| r.seq).unwrap_or(0);
        if newest != self.last_seq {
            let offset = self.log_scroll.offset().y;
            let max = self.log_scroll.max_offset().height;
            if self.last_seq == 0 || -offset >= max - px(24.) {
                self.log_scroll.scroll_to_bottom();
            }
            self.last_seq = newest;
        }
    }

    /// The settings as typed, or why they cannot be used.
    fn edited(&self, cx: &Context<Self>) -> Result<LocalServerSettings, String> {
        let port: u16 = widgets::text(&self.port, cx)
            .parse()
            .ok()
            .filter(|p| *p >= 1024)
            .ok_or("ポートは 1024〜65535 の数字にしてください")?;
        let base_url = widgets::text(&self.base_url, cx);
        if !base_url.is_empty() && !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err("base URL は http:// か https:// で始めてください".into());
        }
        let name = widgets::text(&self.name, cx);
        Ok(LocalServerSettings {
            port,
            base_url: (!base_url.is_empty()).then_some(base_url),
            name: if name.is_empty() { self.local.name.clone() } else { name },
            ..self.local.clone()
        })
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        match self.edited(cx) {
            Ok(local) => {
                self.local = local.clone();
                cx.emit(Nav::Status("保存しました".into(), Tone::Success));
                cx.emit(Nav::LocalChanged(local));
            }
            Err(e) => cx.emit(Nav::Status(e.into(), Tone::Danger)),
        }
        cx.notify();
    }

    fn set_switch(&mut self, change: impl FnOnce(&mut LocalServerSettings), cx: &mut Context<Self>) {
        change(&mut self.local);
        cx.emit(Nav::LocalChanged(self.local.clone()));
        cx.notify();
    }

    fn backup(&mut self, cx: &mut Context<Self>) {
        let task = self.server.update(cx, |s, cx| s.backup(cx));
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(path) => {
                        cx.reveal_path(&path);
                        cx.emit(Nav::Status(
                            format!("バックアップしました: {}", path.display()).into(),
                            Tone::Success,
                        ));
                    }
                    Err(e) => cx.emit(Nav::Status(e.into(), Tone::Danger)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_password(&mut self, cx: &mut Context<Self>) {
        let Some(username) = self.username.clone() else {
            return;
        };
        let password = self.password.read(cx).secret().to_string();
        let task = self.server.update(cx, |s, cx| s.set_password(username, password, cx));
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        this.password.update(cx, |p, cx| p.clear(cx));
                        cx.emit(Nav::Status("パスワードを設定しました".into(), Tone::Success));
                    }
                    Err(e) => cx.emit(Nav::Status(e.into(), Tone::Danger)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn status_card(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let server = self.server.read(cx);
        let status = server.status().clone();
        let set_up = self.username.is_some();
        let needs_restart = server.started_with().is_some_and(|s| *s != self.local);
        let (badge, detail): (Badge, String) = match &status {
            ServerStatus::Stopped => (Badge::new("停止中"), "サーバーは止まっています。".into()),
            ServerStatus::Starting => (Badge::new("起動中…").tone(Tone::Accent), "起動しています…".into()),
            ServerStatus::Stopping => (Badge::new("停止中…"), "止めています…".into()),
            ServerStatus::Running { addr, since } => (
                Badge::new("稼働中").tone(Tone::Success).dot(),
                format!("{addr} で待ち受け中 · {} から", since.format("%m/%d %H:%M")),
            ),
            ServerStatus::Failed(message) => (Badge::new("エラー").tone(Tone::Danger), message.clone()),
        };
        let running = matches!(status, ServerStatus::Running { .. });
        let busy = matches!(status, ServerStatus::Starting | ServerStatus::Stopping);
        // While running, what it was started with; the edited settings apply on a restart.
        let current = server.started_with().unwrap_or(&self.local);
        let url = current.base_url();
        let browser_url = current.client_url();

        let mut actions = div().flex().items_center().gap(theme.spacing.sm);
        if running {
            actions = actions
                .child(
                    Button::new("server-stop", "停止")
                        .icon(IconName::Stop)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Server(ServerAction::Stop)))),
                )
                .child(
                    Button::new("server-restart", "再起動")
                        .icon(IconName::Refresh)
                        .when(needs_restart, |b| b.primary())
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Server(ServerAction::Restart)))),
                )
                .child(
                    Button::new("server-browser", "ブラウザで開く")
                        .ghost()
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&browser_url))),
                );
        } else {
            actions = actions.child(
                Button::new(
                    "server-start",
                    if set_up {
                        "起動"
                    } else {
                        "このコンピューターで始める"
                    },
                )
                .primary()
                .icon(IconName::Play)
                .disabled(busy)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Server(ServerAction::Start)))),
            );
        }

        Card::new()
            .p(theme.spacing.lg)
            .flex()
            .flex_col()
            .gap(theme.spacing.sm)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .child(Label::heading(self.local.name.clone()))
                    .child(badge),
            )
            .child(
                Label::new(detail)
                    .size(TextSize::Small)
                    .color(if matches!(status, ServerStatus::Failed(_)) {
                        TextColor::Danger
                    } else {
                        TextColor::Muted
                    }),
            )
            .when(running, |card| {
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.sm)
                        .child(atelier::SelectableText::new("server-url", url.clone()).mono())
                        .child(CopyButton::new("copy-server-url", url.clone())),
                )
            })
            .when(needs_restart, |card| {
                card.child(
                    Label::new("設定を変えました。再起動すると反映されます。")
                        .size(TextSize::Small)
                        .color(TextColor::Warning),
                )
            })
            .when(!set_up && !running, |card| {
                card.child(
                    Label::new(
                        "専用のサーバーを用意せずに、このコンピューターだけで Knowledge Garden を使えます。初めて起動すると、このコンピューターのアカウント名で管理者を作ってログインします。",
                    )
                    .size(TextSize::Small)
                    .color(TextColor::Muted),
                )
            })
            .child(actions)
    }

    fn log_line(record: &LogRecord) -> LogLine {
        let level = match record.level {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::TRACE => LogLevel::Trace,
        };
        let source = record
            .target
            .strip_prefix("knowledge_garden::")
            .unwrap_or(&record.target)
            .to_string();
        LogLine::new(record.seq.to_string(), record.message.clone())
            .time(record.time.format("%H:%M:%S").to_string())
            .level(level)
            .source(source)
    }
}

impl Render for ServerPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let records = self.server.read(cx).logs().records();
        let skip = records.len().saturating_sub(SHOWN_RECORDS);
        let lines: Vec<LogLine> = records.iter().skip(skip).map(Self::log_line).collect();
        let running = self.server.read(cx).is_running();
        let server = self.server.read(cx);
        let mcp = format!("{}/mcp", server.started_with().unwrap_or(&self.local).client_url());
        let base_url_placeholder = format!("http://localhost:{}", self.local.port);

        let mut page = div()
            .max_w(px(760.))
            .mx_auto()
            .p(theme.spacing.xl)
            .pb(px(64.))
            .flex()
            .flex_col()
            .gap(theme.spacing.lg)
            .child(SectionHeader::new("このコンピューターのサーバー"))
            .child(self.status_card(cx))
            .child(
                FormGroup::new("起動と待ち受け")
                    .child(
                        FormField::new("アプリと一緒に起動する")
                            .description("アプリを開くとサーバーも起動します。アプリを終了すると止まります")
                            .child(Switch::new("autostart", self.local.autostart).on_click(cx.listener(
                                |this, _, _, cx| this.set_switch(|l| l.autostart = !l.autostart, cx),
                            ))),
                    )
                    .child(
                        FormField::new("LAN に公開する")
                            .description(
                                "オフのときはこのコンピューターからだけ使えます。オンにすると同じネットワークの他の機器からも接続できます",
                            )
                            .child(
                                Switch::new("lan", self.local.lan)
                                    .on_click(cx.listener(|this, _, _, cx| this.set_switch(|l| l.lan = !l.lan, cx))),
                            ),
                    )
                    .child(FormField::new("名前").child(widgets::field(&self.name, "手元の Garden", window, cx)))
                    .child(FormField::new("ポート").child(widgets::field(&self.port, "8787", window, cx)))
                    .child(
                        FormField::new("base URL")
                            .description(
                                "記事の URI に使われるので、投稿を始めたあとは変えないでください。空なら http://localhost:<ポート>。LAN や連合で使うときは、他の機器から届く名前（例: http://my-pc.local:8787）にします",
                            )
                            .child(widgets::field(&self.base_url, base_url_placeholder, window, cx)),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new("save-local", "保存")
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        ),
                    ),
            );

        if self.username.is_some() {
            page = page
                .child(
                    FormGroup::new("エージェントをつなぐ")
                        .description(
                            "コーディングエージェントは MCP でこの Garden を読み書きできます。設定画面でエージェント用 API キーを発行すると、登録するコマンドが表示されます。",
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
                                        .child(atelier::SelectableText::new("mcp-url", mcp.clone()).mono()),
                                )
                                .child(CopyButton::new("copy-mcp-url", mcp))
                                .child(
                                    Button::new("issue-agent-key", "API キーを発行…")
                                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Account))),
                                ),
                        ),
                )
                .child(
                    FormGroup::new("データ")
                        .description(format!(
                            "{} に保存しています。バックアップはその中の backups に作ります。",
                            self.local.data_dir().display()
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(theme.spacing.sm)
                                .child(Button::new("open-data", "フォルダを開く").ghost().on_click({
                                    let dir = self.local.data_dir();
                                    cx.listener(move |_, _, _, cx| cx.open_with_system(&dir))
                                }))
                                .child(
                                    Button::new("backup", "バックアップ")
                                        .icon(IconName::Copy)
                                        .disabled(!running || self.busy)
                                        .on_click(cx.listener(|this, _, _, cx| this.backup(cx))),
                                ),
                        ),
                )
                .child(
                    FormGroup::new("ブラウザ用のパスワード")
                        .description(format!(
                            "@{} がブラウザ（他の機器からも）でログインするときのパスワード。このアプリには要りません。",
                            self.username.clone().unwrap_or_default()
                        ))
                        .child(FormField::new("新しいパスワード").child(self.password.clone()))
                        .child(
                            div().flex().justify_end().child(
                                Button::new("set-password", "設定")
                                    .disabled(!running || self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.set_password(cx))),
                            ),
                        ),
                );
        }

        page = page.child(
            FormGroup::new("ログ")
                .child(
                    div()
                        .id("server-log")
                        .h(px(280.))
                        .overflow_y_scroll()
                        .track_scroll(&self.log_scroll)
                        .rounded(theme.shape.radius_md)
                        .border_1()
                        .border_color(theme.colors.border)
                        .p(theme.spacing.xs)
                        .child(if lines.is_empty() {
                            div()
                                .p(theme.spacing.sm)
                                .child(
                                    Label::new("まだ記録はありません")
                                        .size(TextSize::Small)
                                        .color(TextColor::Faint),
                                )
                                .into_any_element()
                        } else {
                            LogView::new("server-log-lines")
                                .lines(lines)
                                .scroll(&self.log_scroll)
                                .source_width(px(72.))
                                .into_any_element()
                        }),
                )
                .child(
                    div().flex().justify_end().child(
                        Button::new("clear-log", "消去")
                            .ghost()
                            .size(ControlSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.server.read(cx).logs().clear();
                                cx.notify();
                            })),
                    ),
                ),
        );
        div().id("server").size_full().overflow_y_scroll().child(page)
    }
}
