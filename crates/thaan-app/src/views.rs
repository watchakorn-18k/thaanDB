//! Tabs that are their own view rather than a table or a query: the Redis key browser
//! (`key_browser.rs`) and the MQTT console (`broker_console.rs`). The app only places
//! them in the tab strip; each view loads its own data off the UI thread. A child
//! module of `app`.

use super::*;
use crate::broker_console::BrokerConsole;
use crate::key_browser::KeyBrowser;
use thaan_core::engine::SessionId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewKind {
    Keys,
    Console,
}

pub(super) struct ViewTab {
    pub(super) ws: SessionId,
    pub(super) sid: SessionId,
    pub(super) title: SharedString,
    pub(super) kind: ViewKind,
    pub(super) view: AnyView,
}

impl ViewKind {
    pub(super) fn icon(self) -> &'static str {
        match self {
            ViewKind::Keys => "key-round",
            ViewKind::Console => "radio-tower",
        }
    }
}

impl ThaanApp {
    /// Opens (or brings forward) the key browser of a Redis database. `on` is the second
    /// session opened for another database; `None` uses the connection's own session.
    pub(super) fn open_keys(
        &mut self,
        on: Option<(SessionId, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_sid() else { return };
        let (sid, database) = match on {
            Some((sid, db)) => (sid, Some(db)),
            None => (ws, None),
        };
        let title = database
            .or_else(|| self.tree.current_database().map(str::to_string))
            .unwrap_or_else(|| "keys".into());
        self.open_view(ws, sid, title.into(), ViewKind::Keys, window, cx);
    }

    /// Opens (or brings forward) the console of the MQTT connection in front.
    pub(super) fn open_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.active_sid() else { return };
        self.open_view(ws, ws, "Console".into(), ViewKind::Console, window, cx);
    }

    fn open_view(
        &mut self,
        ws: SessionId,
        sid: SessionId,
        title: SharedString,
        kind: ViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_home = false;
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::View(v) if v.ws == ws && v.sid == sid && v.kind == kind))
        {
            self.active = i;
            cx.notify();
            return;
        }
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let engine = self.engine.clone();
        let view: AnyView = match kind {
            ViewKind::Keys => cx
                .new(|cx| KeyBrowser::new(engine, sid, read_only, window, cx))
                .into(),
            ViewKind::Console => cx
                .new(|cx| BrokerConsole::new(engine, sid, read_only, window, cx))
                .into(),
        };
        self.tabs.push(Tab::View(Box::new(ViewTab {
            ws,
            sid,
            title,
            kind,
            view,
        })));
        self.active = self.tabs.len() - 1;
        cx.notify();
    }
}
