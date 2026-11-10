//! The main window: the title bar with the Garden switcher, the sidebar, the page in view and
//! the status bar.

use atelier::prelude::*;
use atelier::{
    Divider, Edge, IconName, IconSource, ListItem, MenuItem, Popover, Sidebar, StatusBar, TitleBar, Tone, WindowFrame,
    menu_separator,
};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, KeyBinding, SharedString, Subscription, Window, actions, anchored, deferred, div, point, px,
};
use knowledge_garden::model::EntryKind;

use crate::agent::{AgentPanelView, Article};
use crate::api::{Client, GardenInfo, Me};
use crate::assets::APP_ICON;
use crate::local_server::{LocalServer, ServerEvent, ServerStatus};
use crate::pages::account::AccountPage;
use crate::pages::browse::BrowsePage;
use crate::pages::compose::ComposePage;
use crate::pages::connect::ConnectPage;
use crate::pages::entry::EntryPage;
use crate::pages::server::ServerPage;
use crate::runtime;
use crate::settings::{
    AgentSettings, Connection, ConnectionKind, LOCAL_CONNECTION_ID, LocalServerSettings, Settings, SettingsStore,
};
use crate::update::{UpdateState, Updater};
use crate::widgets::{kind_icon, kind_label};

actions!(garden, [NewEntry, FocusSearch, Back, Reload, ToggleAgent]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-n", NewEntry, None),
        KeyBinding::new("secondary-f", FocusSearch, None),
        KeyBinding::new("secondary-k", FocusSearch, None),
        KeyBinding::new("secondary-r", Reload, None),
        KeyBinding::new("alt-left", Back, None),
        KeyBinding::new("secondary-j", ToggleAgent, None),
    ]);
}

/// The Garden in use: where it is, who is signed in, and how to reach it.
#[derive(Clone)]
pub struct Session {
    pub connection: Connection,
    pub client: Client,
    pub me: Option<Me>,
    pub garden: Option<GardenInfo>,
}

impl Session {
    pub fn signed_in(&self) -> bool {
        self.me.is_some()
    }

    pub fn is_admin(&self) -> bool {
        self.me
            .as_ref()
            .is_some_and(|me| me.user.role == knowledge_garden::model::Role::Admin)
    }

    pub fn username(&self) -> Option<&str> {
        self.me.as_ref().map(|me| me.user.username.as_str())
    }

    /// The address of an entry in the browser.
    pub fn entry_url(&self, id: &str) -> String {
        format!("{}/entries/{id}", self.client.base_url())
    }

    /// The local id of an entry URI of this Garden.
    pub fn local_entry_id<'a>(&self, uri: &'a str) -> Option<&'a str> {
        let base = self
            .garden
            .as_ref()
            .map(|g| g.base_url.as_str())
            .unwrap_or(self.client.base_url());
        uri.strip_prefix(base)
            .or_else(|| uri.strip_prefix(self.client.base_url()))?
            .strip_prefix("/entries/")
            .map(|rest| rest.split(['?', '#', '/']).next().unwrap_or(rest))
            .filter(|id| !id.is_empty())
    }
}

/// What a page asks the window to do.
#[derive(Clone, Debug)]
pub enum Nav {
    OpenEntry(String),
    /// An entry was written; show it in place of the form, with the server's advice.
    Saved {
        id: String,
        warnings: Vec<String>,
    },
    NewEntry,
    EditEntry(String),
    Connect,
    /// A connection was made or changed; switch to it.
    Connected(Connection),
    /// The saved token stopped working or the person signed out.
    SignedOut,
    /// Report what an action did in the status bar.
    Status(SharedString, Tone),
    /// Settings changed on a page (appearance); save and apply them.
    SettingsChanged(Settings),
    /// The local server's settings changed; save them (they apply on the next start).
    LocalChanged(LocalServerSettings),
    Server(ServerAction),
    /// Open the settings page (to issue an agent API key).
    Account,
    /// The entry page finished loading its entry.
    EntryLoaded,
    /// The agent panel's backends or choice changed; save them.
    AgentChanged(AgentSettings),
    ToggleAgent,
    /// Start the downloaded version and quit this one.
    Relaunch,
}

#[derive(Clone, Copy, Debug)]
pub enum ServerAction {
    Start,
    Stop,
    Restart,
}

#[derive(Clone)]
enum Page {
    Browse,
    Entry(Entity<EntryPage>),
    Compose(Entity<ComposePage>),
    Connect,
    Account,
    Server,
}

pub struct GardenApp {
    store: SettingsStore,
    settings: Settings,
    session: Option<Session>,
    /// Checking the saved connection at start-up or after switching.
    connecting: bool,
    page: Page,
    /// Pages to go back to.
    history: Vec<Page>,
    browse: Option<Entity<BrowsePage>>,
    connect: Entity<ConnectPage>,
    account: Option<Entity<AccountPage>>,
    server: Entity<LocalServer>,
    server_page: Option<Entity<ServerPage>>,
    agent: Entity<AgentPanelView>,
    updater: Entity<Updater>,
    /// Switch to the local Garden once its server is up.
    open_when_started: bool,
    status: Option<(SharedString, Tone)>,
    switcher_open: bool,
    /// Holds keyboard focus under the title bar, so the window's shortcuts work before
    /// anything else has been clicked.
    focus: gpui::FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GardenApp {
    pub fn new(store: SettingsStore, settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let connect = cx.new(|cx| ConnectPage::new(store.clone(), settings.clone(), window, cx));
        let mut subscriptions = vec![cx.subscribe_in(&connect, window, Self::on_nav)];
        subscriptions.push(cx.observe_window_appearance(window, |this, window, cx| {
            this.apply_theme(window, cx);
        }));
        let agent = {
            let agent_settings = settings.agent.clone();
            let store = store.clone();
            cx.new(|cx| AgentPanelView::new(agent_settings, store, window, cx))
        };
        subscriptions.push(cx.subscribe_in(&agent, window, Self::on_nav));
        subscriptions.push(cx.observe(&agent, |_, _, cx| cx.notify()));
        let server = cx.new(LocalServer::new);
        subscriptions.push(cx.subscribe_in(&server, window, Self::on_server));
        subscriptions.push(cx.observe(&server, |_, _, cx| cx.notify()));
        // Quitting stops the server, finishing the requests in flight.
        subscriptions.push(cx.on_app_quit(|this, cx| this.server.update(cx, |s, cx| s.stop(cx))));
        let updater = {
            let automatic = settings.update.auto_check;
            cx.new(|cx| Updater::new(automatic, cx))
        };
        subscriptions.push(cx.observe(&updater, |_, _, cx| cx.notify()));
        let mut app = Self {
            store,
            settings,
            session: None,
            connecting: false,
            page: Page::Connect,
            history: Vec::new(),
            browse: None,
            connect,
            account: None,
            server,
            server_page: None,
            agent,
            updater,
            open_when_started: false,
            status: None,
            switcher_open: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        app.focus.focus(window);
        if app.settings.local.autostart && app.settings.local_connection().is_some() {
            app.start_server(cx);
        }
        if let Some(connection) = app.settings.current().cloned() {
            app.open_connection(connection, window, cx);
        }
        app
    }

    /// The admin the app signs in to the local Garden as, and whether it still has a token.
    fn local_access(&self) -> (Option<String>, bool) {
        match self.settings.local_connection() {
            Some(c) => (c.username.clone(), self.store.token(&c.id).is_none()),
            None => (None, true),
        }
    }

    fn start_server(&mut self, cx: &mut Context<Self>) {
        // Entry URIs are built from the base URL: fix it at the first start, so a later change
        // of port does not change them.
        if self.settings.local.base_url.is_none() {
            self.settings.local.base_url = Some(self.settings.local.base_url());
            self.save_settings(cx);
        }
        let (username, need_token) = self.local_access();
        let local = self.settings.local.clone();
        self.server.update(cx, |s, cx| s.start(local, username, need_token, cx));
    }

    fn server_action(&mut self, action: ServerAction, cx: &mut Context<Self>) {
        match action {
            ServerAction::Start => {
                // Show the local Garden once it is up, unless the person is reading another.
                let first = self.settings.local_connection().is_none();
                let here = self
                    .session
                    .as_ref()
                    .is_none_or(|s| s.connection.kind == ConnectionKind::Local);
                self.open_when_started = first || here;
                self.start_server(cx);
            }
            ServerAction::Stop => self.server.update(cx, |s, cx| s.stop(cx)).detach(),
            ServerAction::Restart => {
                let (username, need_token) = self.local_access();
                let local = self.settings.local.clone();
                self.open_when_started = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.connection.kind == ConnectionKind::Local);
                self.server
                    .update(cx, |s, cx| s.restart(local, username, need_token, cx));
            }
        }
    }

    fn on_server(&mut self, _: &Entity<LocalServer>, event: &ServerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.clone() {
            ServerEvent::Started { access } => {
                // Keep the local connection in step with the server: its address, and the new
                // token when one was issued.
                let previous = self.settings.current.clone();
                let existing = self.settings.local_connection().cloned();
                let connection = Connection {
                    id: existing
                        .as_ref()
                        .map(|c| c.id.clone())
                        .unwrap_or_else(|| LOCAL_CONNECTION_ID.into()),
                    name: self.settings.local.name.clone(),
                    base_url: self.settings.local.client_url(),
                    kind: ConnectionKind::Local,
                    username: access
                        .as_ref()
                        .map(|a| a.username.clone())
                        .or_else(|| existing.as_ref().and_then(|c| c.username.clone())),
                };
                if let Some(access) = &access
                    && let Err(e) = self.store.set_token(&connection.id, Some(&access.secret))
                {
                    self.status = Some((format!("トークンを保存できませんでした: {e}").into(), Tone::Danger));
                }
                self.settings.upsert(connection.clone());
                if previous.is_some() {
                    self.settings.current = previous;
                }
                self.save_settings(cx);
                self.connect
                    .update(cx, |c, cx| c.set_settings(self.settings.clone(), cx));
                self.refresh_server_page(cx);
                self.status = Some(("このコンピューターのサーバーが起動しました".into(), Tone::Success));
                if std::mem::take(&mut self.open_when_started) {
                    self.open_connection(connection, window, cx);
                }
            }
            ServerEvent::Failed(message) => {
                self.status = Some((message.into(), Tone::Danger));
                if std::mem::take(&mut self.open_when_started) {
                    self.connecting = false;
                    self.server_page(window, cx);
                }
                cx.notify();
            }
            ServerEvent::Stopped => {
                self.status = Some(("このコンピューターのサーバーを止めました".into(), Tone::Neutral));
                cx.notify();
            }
        }
    }

    fn refresh_server_page(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = &self.server_page {
            let local = self.settings.local.clone();
            let username = self.settings.local_connection().and_then(|c| c.username.clone());
            page.update(cx, |p, cx| p.update_from(local, username, cx));
        }
    }

    fn server_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.server_page.is_none() {
            let server = self.server.clone();
            let local = self.settings.local.clone();
            let username = self.settings.local_connection().and_then(|c| c.username.clone());
            let page = cx.new(|cx| ServerPage::new(server, local, username, window, cx));
            self._subscriptions.push(cx.subscribe_in(&page, window, Self::on_nav));
            self.server_page = Some(page);
        }
        if !matches!(self.page, Page::Server) {
            self.go(Page::Server, cx);
        }
    }

    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = crate::theme_for(
            &self.settings.direction,
            self.settings.appearance,
            crate::is_dark(window.appearance()),
        );
        atelier::set_window_background(window, &theme);
        theme.install(cx);
        cx.notify();
    }

    /// Stops the local server (so the new version can take its port), then starts the new
    /// version and quits.
    fn relaunch(&mut self, cx: &mut Context<Self>) {
        let stopped = self.server.update(cx, |s, cx| s.stop(cx));
        cx.spawn(async move |this, cx| {
            stopped.await;
            this.update(cx, |this, cx| match this.updater.read(cx).relaunch() {
                Ok(()) => cx.quit(),
                Err(e) => {
                    this.status = Some((e.into(), Tone::Danger));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.store.save(&self.settings) {
            self.status = Some((format!("設定を保存できませんでした: {e}").into(), Tone::Danger));
            cx.notify();
        }
    }

    /// Switches to `connection`: checks the Garden and the saved token, then shows its entries.
    fn open_connection(&mut self, connection: Connection, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.current = Some(connection.id.clone());
        self.save_settings(cx);
        self.connecting = true;
        self.switcher_open = false;
        // The local Garden answers only while its server runs: start it, and come back here.
        if connection.kind == ConnectionKind::Local && !self.server.read(cx).is_running() {
            self.open_when_started = true;
            if !self.server.read(cx).is_busy() {
                self.start_server(cx);
            }
            cx.notify();
            return;
        }
        let token = self.store.token(&connection.id);
        let client = Client::new(&connection.base_url, token);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let probe = client.clone();
            let (garden, me) = runtime::run(async move {
                let garden = probe.garden().await;
                let me = if probe.has_token() {
                    Some(probe.me().await)
                } else {
                    None
                };
                (garden, me)
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                this.connecting = false;
                let garden = match garden {
                    Ok(g) => Some(g),
                    Err(e) => {
                        this.status = Some((format!("{}: {e}", connection.name).into(), Tone::Danger));
                        None
                    }
                };
                let me = match me {
                    Some(Ok(me)) => Some(me),
                    Some(Err(e)) if e.is_unauthorized() => {
                        this.status = Some((
                            "保存したトークンが使えなくなりました。もう一度ログインしてください".into(),
                            Tone::Warning,
                        ));
                        None
                    }
                    _ => None,
                };
                // Keep a token whose check failed for another reason (the Garden is down).
                let client = if me.is_none() && garden.is_some() && client.has_token() {
                    client.with_token(None)
                } else {
                    client
                };
                let session = Session {
                    connection,
                    client,
                    me,
                    garden,
                };
                this.start_session(session, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn start_session(&mut self, session: Session, window: &mut Window, cx: &mut Context<Self>) {
        let browse = cx.new(|cx| BrowsePage::new(session.clone(), window, cx));
        self._subscriptions.push(cx.subscribe_in(&browse, window, Self::on_nav));
        self.browse = Some(browse);
        self.account = None;
        let for_agent = session.clone();
        self.agent.update(cx, |a, cx| a.set_session(Some(for_agent), cx));
        self.session = Some(session);
        self.history.clear();
        self.page = Page::Browse;
        self.sync_article(cx);
        cx.notify();
    }

    /// Tells the agent panel which entry is in view.
    fn sync_article(&mut self, cx: &mut Context<Self>) {
        let article = match &self.page {
            Page::Entry(page) => page.read(cx).entry().map(Article::from_entry),
            _ => None,
        };
        self.agent.update(cx, |a, cx| a.set_article(article, cx));
    }

    fn toggle_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.agent.panel_open = !self.settings.agent.panel_open;
        self.save_settings(cx);
        if self.settings.agent.panel_open {
            self.agent.update(cx, |a, cx| a.focus(window, cx));
        }
        cx.notify();
    }

    fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        let previous = std::mem::replace(&mut self.page, page);
        self.history.push(previous);
        if self.history.len() > 50 {
            self.history.remove(0);
        }
        self.sync_article(cx);
        cx.notify();
    }

    fn back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = self.history.pop() {
            self.page = page;
            self.sync_article(cx);
            cx.notify();
        }
    }

    fn open_entry(&mut self, id: String, warnings: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let page = cx.new(|cx| EntryPage::new(session, id, window, cx).with_warnings(warnings));
        self._subscriptions.push(cx.subscribe_in(&page, window, Self::on_nav));
        self.go(Page::Entry(page), cx);
    }

    fn compose(&mut self, edit: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if !session.signed_in() {
            self.status = Some(("投稿するにはログインしてください".into(), Tone::Warning));
            self.go(Page::Connect, cx);
            return;
        }
        let page = cx.new(|cx| ComposePage::new(session, edit, window, cx));
        self._subscriptions.push(cx.subscribe_in(&page, window, Self::on_nav));
        self.go(Page::Compose(page), cx);
    }

    fn account_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = match &self.account {
            Some(page) => page.clone(),
            None => {
                let session = self.session.clone();
                let settings = self.settings.clone();
                let updater = self.updater.clone();
                let page = cx.new(|cx| AccountPage::new(session, settings, updater, window, cx));
                self._subscriptions.push(cx.subscribe_in(&page, window, Self::on_nav));
                self.account = Some(page.clone());
                page
            }
        };
        page.update(cx, |page, cx| page.refresh(cx));
        self.go(Page::Account, cx);
    }

    fn on_nav<E>(&mut self, _: &Entity<E>, nav: &Nav, window: &mut Window, cx: &mut Context<Self>) {
        match nav.clone() {
            Nav::OpenEntry(id) => self.open_entry(id, Vec::new(), window, cx),
            Nav::Saved { id, warnings } => {
                // The form is done with: going back skips it.
                if matches!(self.page, Page::Compose(_)) {
                    self.page = self.history.pop().unwrap_or(Page::Browse);
                    if let Page::Entry(old) = &self.page
                        && old.read(cx).id() == id
                    {
                        self.page = self.history.pop().unwrap_or(Page::Browse);
                    }
                }
                if let Some(browse) = &self.browse {
                    browse.update(cx, |b, cx| b.reload(cx));
                }
                self.open_entry(id, warnings, window, cx);
            }
            Nav::NewEntry => self.compose(None, window, cx),
            Nav::EditEntry(id) => self.compose(Some(id), window, cx),
            Nav::Connect => self.go(Page::Connect, cx),
            Nav::Connected(connection) => {
                self.settings = self.store.load();
                self.connect
                    .update(cx, |c, cx| c.set_settings(self.settings.clone(), cx));
                self.open_connection(connection, window, cx);
            }
            Nav::SignedOut => {
                if let Some(session) = &self.session {
                    let _ = self.store.set_token(&session.connection.id, None);
                    let connection = session.connection.clone();
                    self.status = Some(("ログアウトしました".into(), Tone::Neutral));
                    self.open_connection(connection, window, cx);
                }
            }
            Nav::Status(message, tone) => {
                self.status = Some((message, tone));
                cx.notify();
            }
            Nav::LocalChanged(local) => {
                self.settings.local = local;
                self.save_settings(cx);
                cx.notify();
            }
            Nav::Server(action) => self.server_action(action, cx),
            Nav::Account => self.account_page(window, cx),
            Nav::EntryLoaded => self.sync_article(cx),
            Nav::AgentChanged(agent) => {
                // Whether the panel is open is the window's to say.
                let open = self.settings.agent.panel_open;
                self.settings.agent = AgentSettings {
                    panel_open: open,
                    ..agent
                };
                self.save_settings(cx);
            }
            Nav::ToggleAgent => self.toggle_agent(window, cx),
            Nav::Relaunch => self.relaunch(cx),
            Nav::SettingsChanged(settings) => {
                // Only what the settings page owns: its copy of the rest may be stale.
                self.settings.appearance = settings.appearance;
                self.settings.direction = settings.direction;
                self.settings.update = settings.update;
                self.save_settings(cx);
                self.connect
                    .update(cx, |c, cx| c.set_settings(self.settings.clone(), cx));
                self.apply_theme(window, cx);
            }
        }
    }

    fn is_page(&self, which: &str) -> bool {
        matches!(
            (&self.page, which),
            (Page::Browse, "browse")
                | (Page::Compose(_), "compose")
                | (Page::Connect, "connect")
                | (Page::Account, "account")
                | (Page::Server, "server")
        )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let filter_kind = self.browse.as_ref().and_then(|b| b.read(cx).kind());
        let browsing = self.is_page("browse");
        let signed_in = self.session.as_ref().is_some_and(Session::signed_in);
        let mut sidebar = Sidebar::new(Edge::Left)
            .id("sidebar")
            .overflow_y_scroll()
            .w(px(232.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme.spacing.sm)
            .child(
                ListItem::new("nav-browse", "記事")
                    .icon(IconName::Menu)
                    .selected(browsing && filter_kind.is_none())
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(b) = &this.browse {
                            b.update(cx, |b, cx| b.set_kind(None, window, cx));
                        }
                        this.go(Page::Browse, cx);
                    })),
            );
        for (i, kind) in EntryKind::ALL.iter().copied().enumerate() {
            sidebar = sidebar.child(
                ListItem::new(("nav-kind", i), kind_label(kind))
                    .icon(kind_icon(kind))
                    .indent(1)
                    .selected(browsing && filter_kind == Some(kind))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(b) = &this.browse {
                            b.update(cx, |b, cx| b.set_kind(Some(kind), window, cx));
                        }
                        this.go(Page::Browse, cx);
                    })),
            );
        }
        sidebar = sidebar
            .child(div().h(theme.spacing.sm))
            .child(
                ListItem::new("nav-compose", "新規投稿")
                    .icon(IconName::Plus)
                    .disabled(!signed_in)
                    .selected(self.is_page("compose"))
                    .on_click(cx.listener(|this, _, window, cx| this.compose(None, window, cx))),
            )
            .child(div().flex_1())
            .child(
                ListItem::new("nav-server", "サーバー")
                    .icon(IconName::Sliders)
                    .selected(self.is_page("server"))
                    .trailing(match self.server.read(cx).status() {
                        ServerStatus::Running { .. } => {
                            Badge::new("稼働中").tone(Tone::Success).dot().into_any_element()
                        }
                        ServerStatus::Starting => Badge::new("起動中").tone(Tone::Accent).into_any_element(),
                        ServerStatus::Failed(_) => Badge::new("エラー").tone(Tone::Danger).into_any_element(),
                        _ => div().into_any_element(),
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.server_page(window, cx))),
            )
            .child(
                ListItem::new("nav-connect", "接続先")
                    .icon(IconName::Folder)
                    .selected(self.is_page("connect"))
                    .on_click(cx.listener(|this, _, _, cx| this.go(Page::Connect, cx))),
            )
            .child(
                ListItem::new("nav-account", "設定")
                    .icon(IconName::Settings)
                    .selected(self.is_page("account"))
                    .on_click(cx.listener(|this, _, window, cx| this.account_page(window, cx))),
            );
        sidebar
    }

    fn switcher(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = self.settings.current.clone();
        let mut menu = Popover::new().w(px(280.));
        for (i, connection) in self.settings.connections.iter().enumerate() {
            let c = connection.clone();
            menu = menu.child(
                MenuItem::new(("garden", i), connection.name.clone())
                    .checked(current.as_deref() == Some(connection.id.as_str()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_connection(c.clone(), window, cx);
                    })),
            );
        }
        menu.child(menu_separator(cx)).child(
            MenuItem::new("garden-add", "Garden を追加…")
                .icon(IconName::Plus)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.switcher_open = false;
                    this.go(Page::Connect, cx);
                })),
        )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> StatusBar {
        let theme = cx.theme().clone();
        let who = match &self.session {
            Some(s) => match s.username() {
                Some(name) => format!("{} · {name}", s.client.base_url()),
                None => format!("{} · ゲスト", s.client.base_url()),
            },
            None => "未接続".into(),
        };
        let (message, tone) = self.status.clone().unwrap_or_else(|| {
            (
                if self.connecting {
                    "接続しています…".into()
                } else {
                    "".into()
                },
                Tone::Neutral,
            )
        });
        let update = match self.updater.read(cx).state() {
            UpdateState::Ready(available) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .child(
                        Label::new(format!("バージョン {} の準備ができました", available.version))
                            .size(TextSize::XSmall)
                            .color(TextColor::Accent),
                    )
                    .child(
                        Button::new("relaunch", "再起動して更新")
                            .size(ControlSize::Small)
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.relaunch(cx))),
                    ),
            ),
            UpdateState::Manual(available) => {
                let page = available.page();
                Some(
                    div().flex().items_center().child(
                        Button::new("release-page", format!("バージョン {} が出ています", available.version))
                            .size(ControlSize::Small)
                            .ghost()
                            .icon(IconName::ArrowUp)
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    ),
                )
            }
            _ => None,
        };
        StatusBar::new()
            .message(message)
            .tone(tone)
            .busy(self.connecting)
            .trailing(div().children(update))
            .trailing(Label::new(who).size(TextSize::XSmall).color(TextColor::Muted))
            .trailing(div().w(theme.spacing.sm))
    }
}

impl Render for GardenApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let garden_name: SharedString = self
            .session
            .as_ref()
            .map(|s| s.connection.name.clone())
            .unwrap_or_else(|| "Garden を選ぶ".into())
            .into();
        let content: gpui::AnyElement = match &self.page {
            Page::Browse => match &self.browse {
                Some(b) => b.clone().into_any_element(),
                None => self.connect.clone().into_any_element(),
            },
            Page::Entry(e) => e.clone().into_any_element(),
            Page::Compose(c) => c.clone().into_any_element(),
            Page::Connect => self.connect.clone().into_any_element(),
            Page::Account => match &self.account {
                Some(a) => a.clone().into_any_element(),
                None => self.connect.clone().into_any_element(),
            },
            Page::Server => match &self.server_page {
                Some(p) => p.clone().into_any_element(),
                None => self.connect.clone().into_any_element(),
            },
        };
        let switcher = self.switcher_open.then(|| self.switcher(cx));
        let _ = window;
        WindowFrame::new()
            .font(theme.typography.ui.clone())
            .text_color(theme.colors.text)
            .size_full()
            .flex()
            .flex_col()
            .child(
                TitleBar::new()
                    .leading(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.xs)
                            .child(Avatar::logo(IconSource::image(APP_ICON)).size(px(20.)))
                            .child(
                                Button::new("garden-switcher", garden_name)
                                    .ghost()
                                    .size(ControlSize::Small)
                                    .icon_after(IconName::ChevronDown)
                                    .tooltip("接続先の Garden を切り替える")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.switcher_open = !this.switcher_open;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .leading(
                        Button::icon_only("back", IconName::ChevronLeft)
                            .size(ControlSize::Small)
                            .ghost()
                            .disabled(self.history.is_empty())
                            .tooltip("戻る")
                            .shortcut_for(&Back)
                            .on_click(cx.listener(|this, _, window, cx| this.back(&Back, window, cx))),
                    )
                    .title("Knowledge Garden")
                    .trailing(
                        atelier::agent::AgentToggle::new(
                            "agent-toggle",
                            self.settings.agent.panel_open,
                            self.agent.read(cx).status(),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_agent(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("garden-body")
                    .key_context("GardenApp")
                    .track_focus(&self.focus)
                    .on_action(cx.listener(Self::back))
                    .on_action(cx.listener(|this, _: &ToggleAgent, window, cx| this.toggle_agent(window, cx)))
                    .on_action(cx.listener(|this, _: &NewEntry, window, cx| this.compose(None, window, cx)))
                    .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                        if let Some(b) = this.browse.clone() {
                            this.go(Page::Browse, cx);
                            b.update(cx, |b, cx| b.focus_search(window, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Reload, window, cx| match &this.page {
                        Page::Browse => {
                            if let Some(b) = &this.browse {
                                b.update(cx, |b, cx| b.reload(cx));
                            }
                        }
                        Page::Entry(e) => e.update(cx, |e, cx| e.reload(window, cx)),
                        _ => {}
                    }))
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(Divider::vertical().full())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .bg(theme.colors.surface)
                            .child(content),
                    )
                    .when(self.settings.agent.panel_open, |body| {
                        body.child(Divider::vertical().full()).child(
                            div()
                                .w(crate::agent::PANEL_WIDTH)
                                .flex_none()
                                .h_full()
                                .child(self.agent.clone()),
                        )
                    }),
            )
            .child(self.status_bar(cx))
            .children(switcher.map(|menu| {
                deferred(
                    anchored().position(point(px(40.), theme.density.title_bar)).child(
                        div()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.switcher_open = false;
                                cx.notify();
                            }))
                            .child(menu),
                    ),
                )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ConnectionKind;

    fn session(base: &str, garden_base: &str) -> Session {
        Session {
            connection: Connection {
                id: "a".into(),
                name: "NAS".into(),
                base_url: base.into(),
                kind: ConnectionKind::Remote,
                username: None,
            },
            client: Client::new(base, None),
            me: None,
            garden: Some(GardenInfo {
                name: "NAS".into(),
                base_url: garden_base.into(),
                guest_read: true,
                registration: "invite".into(),
                default_visibility: knowledge_garden::model::Visibility::Public,
            }),
        }
    }

    #[test]
    fn entry_links_of_this_garden_open_in_the_app() {
        // Reached through an IP address, while entry URIs use the Garden's public name.
        let s = session("http://192.168.1.20:8080", "http://nas.local:8080");
        assert_eq!(
            s.local_entry_id("http://nas.local:8080/entries/0192-abc"),
            Some("0192-abc")
        );
        assert_eq!(
            s.local_entry_id("http://192.168.1.20:8080/entries/0192-abc#comments"),
            Some("0192-abc")
        );
        assert_eq!(s.local_entry_id("https://other.example/entries/0192-abc"), None);
        assert_eq!(s.local_entry_id("http://nas.local:8080/entries/"), None);
    }
}
