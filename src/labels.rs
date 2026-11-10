//! Japanese names of the entry kinds, states, visibilities, relations and
//! reproduction results, shared by the web UI and the desktop app.

use crate::model::*;

pub fn kind_label(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Memo => "メモ",
        EntryKind::ResearchNote => "研究ノート",
        EntryKind::ResearchReport => "研究レポート",
        EntryKind::Troubleshooting => "トラブルシューティング",
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
