//! The app's own files, served to gpui after Atelier's icon set.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

/// The Kn mark (Atelier element cell, number 3) in its small drawing, for
/// `TitleBar::app_icon`: the window's top left on Windows and Linux.
pub const TITLE_BAR_ICON: &str = "app/title-bar.png";

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            TITLE_BAR_ICON => Some(Cow::Borrowed(include_bytes!(
                "../../assets/app-icons/knowledge-garden/title-bar.png"
            ))),
            _ => None,
        })
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![TITLE_BAR_ICON.into()])
    }
}
