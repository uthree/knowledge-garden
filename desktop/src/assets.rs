//! The app's own files, served to gpui after Atelier's icon set.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

/// The Kn mark (Atelier element cell, number 3), as in the web UI's header.
pub const APP_ICON: &str = "app/icon.png";

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            APP_ICON => Some(Cow::Borrowed(include_bytes!(
                "../../assets/app-icons/knowledge-garden/png/128.png"
            ))),
            _ => None,
        })
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(vec![APP_ICON.into()])
    }
}
