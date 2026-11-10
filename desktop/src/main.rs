//! Knowledge Garden for the desktop: read, search and write a Garden's entries, built with gpui
//! and Atelier. See `docs/desktop.md` for the design.

#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod api;
mod app;
mod assets;
mod local_server;
mod logs;
mod pages;
mod runtime;
mod settings;
mod widgets;

use atelier::{Appearance, Assets, Direction, Strings, Theme};
use gpui::{App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use crate::settings::{AppearanceChoice, SettingsStore};

/// The smallest window the layout fits in without cutting off controls.
const MIN_WIDTH: f32 = 880.;
const MIN_HEIGHT: f32 = 560.;

/// The theme for the saved choice, following the system when asked to.
pub fn theme_for(direction: &str, choice: AppearanceChoice, system_dark: bool) -> Theme {
    let direction = Direction::from_id(direction).unwrap_or_default();
    let appearance = match choice {
        AppearanceChoice::Light => Appearance::Light,
        AppearanceChoice::Dark => Appearance::Dark,
        AppearanceChoice::System if system_dark => Appearance::Dark,
        AppearanceChoice::System => Appearance::Light,
    };
    Theme::new(direction, appearance)
}

pub fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

fn main() {
    logs::init();
    let store = SettingsStore::default_location();
    let settings = store.load();
    Application::new()
        .with_assets(Assets::with_fallback(assets::AppAssets))
        .run(move |cx: &mut App| {
            let theme = theme_for(
                &settings.direction,
                settings.appearance,
                is_dark(cx.window_appearance()),
            );
            let background = theme.window_background();
            atelier::init(cx, theme.clone());
            Strings::japanese().install(cx);
            app::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = atelier::custom_frame(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
                    window_background: background,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Knowledge Garden".into()),
                        ..Default::default()
                    }),
                    app_id: Some("knowledge-garden-desktop".into()),
                    ..Default::default()
                },
                &theme,
            );
            cx.open_window(options, |window, cx| {
                atelier::set_window_background(window, &theme);
                cx.new(|cx| app::GardenApp::new(store, settings, window, cx))
            })
            .expect("failed to open the window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
