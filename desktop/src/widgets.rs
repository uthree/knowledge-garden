//! Small pieces shared by the pages: names and marks for the Garden's vocabulary, text fields
//! and a password field.

use atelier::prelude::*;
use atelier::{IconName, TextField, TextInput, Tone};
use gpui::prelude::*;
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, SharedString,
    Window, div,
};
use knowledge_garden::model::{EntryKind, ReproResult, Status, TokenKind};
use unicode_segmentation::UnicodeSegmentation;

pub use knowledge_garden::labels::{kind_label, relation_label, repro_label, status_label, visibility_label};

pub fn kind_icon(kind: EntryKind) -> IconName {
    match kind {
        EntryKind::Memo => IconName::Bolt,
        EntryKind::ResearchNote => IconName::Brain,
        EntryKind::ResearchReport => IconName::File,
        EntryKind::Troubleshooting => IconName::Wrench,
    }
}

pub fn status_tone(status: Status) -> Tone {
    match status {
        Status::Verified => Tone::Success,
        Status::Retracted => Tone::Danger,
        Status::Superseded => Tone::Warning,
        _ => Tone::Neutral,
    }
}

pub fn repro_tone(result: ReproResult) -> Tone {
    match result {
        ReproResult::Reproduced => Tone::Success,
        ReproResult::Partial => Tone::Warning,
        ReproResult::Failed => Tone::Danger,
    }
}

/// "AI" for posts made with an agent token, so readers can tell them apart.
pub fn posted_via_badge(kind: TokenKind, name: &str) -> Option<Badge> {
    (kind == TokenKind::Agent).then(|| Badge::new(format!("AI · {name}")).tone(Tone::Agent))
}

/// A local date and time, as people read it.
pub fn local_time(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()
}

/// A single-line input that starts empty.
pub fn input(cx: &mut App) -> Entity<TextInput> {
    cx.new(TextInput::new)
}

/// A single-line input holding `text`.
pub fn input_with(text: &str, cx: &mut App) -> Entity<TextInput> {
    let text = text.to_string();
    cx.new(|cx| TextInput::new(cx).with_text(text, cx))
}

/// An input for several lines (a body, a comment).
pub fn area_input(text: &str, cx: &mut App) -> Entity<TextInput> {
    let text = text.to_string();
    cx.new(|cx| TextInput::new(cx).multiline().with_text(text, cx))
}

/// The frame around an input, with its placeholder and focus ring.
pub fn field(input: &Entity<TextInput>, placeholder: impl Into<SharedString>, window: &Window, cx: &App) -> TextField {
    let state = input.read(cx);
    TextField::new()
        .placeholder(placeholder)
        .empty(state.is_empty())
        .focused(state.is_focused(window))
        .child(input.clone())
}

/// The same, for several lines.
pub fn area(input: &Entity<TextInput>, placeholder: impl Into<SharedString>, window: &Window, cx: &App) -> TextField {
    field(input, placeholder, window, cx).area()
}

/// The text of an input, trimmed.
pub fn text(input: &Entity<TextInput>, cx: &App) -> String {
    input.read(cx).text().trim().to_string()
}

/// Return was pressed in a [`PasswordField`].
pub struct PasswordSubmit;

/// A field whose text is shown as dots. Atelier's `TextInput` always shows its text, so this
/// one takes keys itself: typing, Backspace, paste and Return. Input methods do not compose
/// here, which suits a password.
pub struct PasswordField {
    focus: FocusHandle,
    secret: String,
}

impl EventEmitter<PasswordSubmit> for PasswordField {}

impl Focusable for PasswordField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PasswordField {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            secret: String::new(),
        }
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.secret.clear();
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        match keystroke.key.as_str() {
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.secret.push_str(text.trim_end_matches(['\r', '\n']));
                }
            }
            "c" | "x" if command => {
                // A password is never copied out; clear the clipboard's stale copy instead.
                cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                return;
            }
            "enter" => {
                cx.emit(PasswordSubmit);
                return;
            }
            "backspace" => {
                if let Some((at, _)) = self.secret.grapheme_indices(true).next_back() {
                    self.secret.truncate(at);
                }
            }
            _ if command || keystroke.modifiers.alt => return,
            "tab" | "escape" => return,
            _ => match keystroke.key_char.as_deref() {
                Some(typed) => self.secret.push_str(typed),
                None => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for PasswordField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dots: String = "•".repeat(self.secret.graphemes(true).count());
        TextField::new()
            .id("password")
            .track_focus(&self.focus)
            .key_context("PasswordField")
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus.focus(window)),
            )
            .placeholder("パスワード")
            .empty(self.secret.is_empty())
            .focused(self.focus.is_focused(window))
            .child(div().child(dots))
    }
}
