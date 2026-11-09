//! Who may read and change what. Single-entry checks are done here in Rust;
//! listing queries use [`VisibilityFilter`], which encodes the same rules.

use crate::config::Config;
use crate::model::{EntryAccess, Principal, Scope, Visibility};

/// A caller counts as a member reader when its token has the `read` scope.
/// A token without it reads like a guest (it can still read its own entries).
fn is_member_reader(viewer: Option<&Principal>) -> bool {
    viewer.is_some_and(|p| p.has(Scope::Read))
}

fn is_author(viewer: Option<&Principal>, author_id: &str) -> bool {
    viewer.is_some_and(|p| p.user.id == author_id)
}

/// Whether the caller may read a single entry (by id or link).
pub fn can_read(config: &Config, viewer: Option<&Principal>, entry: &EntryAccess) -> bool {
    if is_author(viewer, &entry.author_id) {
        return true;
    }
    let member = is_member_reader(viewer);
    match entry.visibility {
        Visibility::Public | Visibility::Unlisted => member || config.guest_read,
        // Federated entries will also be readable by approved peer Gardens once federation lands.
        Visibility::Federated | Visibility::Members => member,
        Visibility::Private => false,
    }
}

/// Whether the caller may change an entry's content or metadata.
/// Mirrored entries can only be changed by their origin Garden.
pub fn can_edit(viewer: &Principal, entry: &EntryAccess) -> bool {
    entry.is_local && viewer.has(Scope::EntriesWrite) && (viewer.user.id == entry.author_id || viewer.is_admin())
}

/// Which entries appear in lists, tag counts and backlinks for a caller:
/// their own entries, plus entries with one of `visible` visibilities.
/// Unlisted entries never appear in lists for anyone but the author.
#[derive(Debug, Clone)]
pub struct VisibilityFilter {
    pub viewer_id: Option<String>,
    pub visible: Vec<Visibility>,
}

impl VisibilityFilter {
    pub fn for_viewer(config: &Config, viewer: Option<&Principal>) -> Self {
        let visible = if is_member_reader(viewer) {
            vec![Visibility::Public, Visibility::Federated, Visibility::Members]
        } else if config.guest_read {
            vec![Visibility::Public]
        } else {
            vec![]
        };
        Self {
            viewer_id: viewer.map(|p| p.user.id.clone()),
            visible,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Role, Token, TokenKind, User};
    use chrono::Utc;

    fn principal(id: &str, scopes: &[Scope], role: Role) -> Principal {
        Principal {
            user: User {
                id: id.into(),
                username: id.into(),
                display_name: id.into(),
                role,
                created_at: Utc::now(),
            },
            token: Token {
                id: format!("tok-{id}"),
                user_id: id.into(),
                name: "t".into(),
                kind: TokenKind::Agent,
                scopes: scopes.to_vec(),
                created_at: Utc::now(),
                last_used_at: None,
                revoked_at: None,
            },
        }
    }

    fn entry(visibility: Visibility) -> EntryAccess {
        EntryAccess {
            id: "e".into(),
            uri: "u".into(),
            visibility,
            author_id: "author".into(),
            current_version: 1,
            is_local: true,
        }
    }

    #[test]
    fn read_matrix() {
        let config = Config::new("http://localhost", "test");
        let member = principal("m", &[Scope::Read], Role::Member);
        let no_read = principal("n", &[Scope::EntriesWrite], Role::Member);
        let author = principal("author", &[], Role::Member);

        for (vis, guest, mem, noread) in [
            (Visibility::Public, true, true, true),
            (Visibility::Unlisted, true, true, true),
            (Visibility::Federated, false, true, false),
            (Visibility::Members, false, true, false),
            (Visibility::Private, false, false, false),
        ] {
            let e = entry(vis);
            assert_eq!(can_read(&config, None, &e), guest, "guest {vis}");
            assert_eq!(can_read(&config, Some(&member), &e), mem, "member {vis}");
            assert_eq!(can_read(&config, Some(&no_read), &e), noread, "no-read {vis}");
            assert!(can_read(&config, Some(&author), &e), "author {vis}");
        }
    }

    #[test]
    fn guests_blocked_when_guest_read_disabled() {
        let mut config = Config::new("http://localhost", "test");
        config.guest_read = false;
        assert!(!can_read(&config, None, &entry(Visibility::Public)));
        assert!(VisibilityFilter::for_viewer(&config, None).visible.is_empty());
    }

    #[test]
    fn admin_can_edit_others() {
        let admin = principal("a", &[Scope::EntriesWrite, Scope::Admin], Role::Admin);
        let other = principal("o", &[Scope::EntriesWrite], Role::Member);
        let e = entry(Visibility::Public);
        assert!(can_edit(&admin, &e));
        assert!(!can_edit(&other, &e));
    }
}
