//! Messages from Luau panels and hooks (`host.notify`, refused actions and
//! hook errors), as short-lived cards in the notification stack.
use crate::theme::*;
use gpui::{Context, FontWeight, Render, Role, Task, Window, div, prelude::*, px};
use std::time::Duration;

const SHOWN_FOR: Duration = Duration::from_secs(6);
const MAX_SHOWN: usize = 3;

#[derive(Clone, Debug, PartialEq)]
struct Message {
    id: u64,
    source: String,
    text: String,
}

#[derive(Default)]
pub(crate) struct PanelNotice {
    messages: Vec<Message>,
    next: u64,
    _expiry: Vec<Task<()>>,
}

impl PanelNotice {
    pub(crate) fn push(&mut self, source: String, text: String, cx: &mut Context<Self>) {
        let text: String = text.chars().take(400).collect();
        if text.trim().is_empty() {
            return;
        }
        self.next += 1;
        let id = self.next;
        self.messages.push(Message { id, source, text });
        if self.messages.len() > MAX_SHOWN {
            self.messages.remove(0);
        }
        self._expiry.retain(|t| !t.is_ready());
        self._expiry.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOWN_FOR).await;
            let _ = this.update(cx, |this, cx| this.dismiss(id, cx));
        }));
        cx.notify();
    }
    fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.messages.len();
        self.messages.retain(|m| m.id != id);
        if self.messages.len() != before {
            cx.notify();
        }
    }
    pub(crate) fn is_visible(&self) -> bool {
        !self.messages.is_empty()
    }
}

impl Render for PanelNotice {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(self.messages.iter().map(|message| {
                let id = message.id;
                super::surface::toast_frame()
                    .id(("panel-notice", id as usize))
                    .debug_selector(|| "panel-notice".into())
                    .role(Role::Status)
                    .child(
                        div()
                            .px(px(14.))
                            .py_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(icon("dashboard", 14.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(message.source.clone()),
                                    )
                                    .child(
                                        icon_button("dismiss-panel-notice", "Dismiss", "close")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.dismiss(id, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(content(0.85))
                                    .child(message.text.clone()),
                            ),
                    )
            }))
    }
}
