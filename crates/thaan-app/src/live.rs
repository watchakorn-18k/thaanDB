//! Several live connections at once. The connection in front keeps its session, schema
//! and tree in the `ThaanApp` fields (`session`, `objects`, `tree`); every other open
//! connection waits here, and switching only swaps the two (no reconnect, no reload).
//!
//! Tabs are shared: one tab strip holds the tabs of every connection, and each tab
//! remembers the connection it belongs to (`Tab::ws`). The connection in front follows
//! the tab on screen, so the title, status bar and Run always match what is shown.
//! Background answers find their tab by connection and index, so a result never lands
//! in another connection's tab. A child module of `app`.

use super::*;
use thaan_core::engine::SessionId;

/// An open connection that is not the one in front.
pub(super) struct Parked {
    pub(super) session: SessionInfo,
    pub(super) objects: Vec<SchemaObject>,
    pub(super) tree: ObjectTree,
}

impl ThaanApp {
    /// The session in front, if any.
    pub(super) fn active_sid(&self) -> Option<SessionId> {
        self.session.as_ref().map(|s| s.session_id)
    }

    /// The live session opened from saved connection `connection_id`.
    pub(super) fn live_for(&self, connection_id: &str) -> Option<SessionId> {
        if connection_id.is_empty() {
            return None;
        }
        self.session
            .iter()
            .chain(self.parked.iter().map(|p| &p.session))
            .find(|s| s.connection_id == connection_id)
            .map(|s| s.session_id)
    }

    /// Whether `sid` is an open connection (in front or parked).
    pub(super) fn is_live(&self, sid: SessionId) -> bool {
        self.active_sid() == Some(sid) || self.parked.iter().any(|p| p.session.session_id == sid)
    }

    /// The open connection `sid`, in front or parked.
    pub(super) fn session_info(&self, sid: SessionId) -> Option<&SessionInfo> {
        self.session
            .iter()
            .chain(self.parked.iter().map(|p| &p.session))
            .find(|s| s.session_id == sid)
    }

    /// Moves the connection in front (if any) to the parked list.
    fn park_active(&mut self) {
        if let Some(session) = self.session.take() {
            self.parked.push(Parked {
                session,
                objects: std::mem::take(&mut self.objects),
                tree: std::mem::take(&mut self.tree),
            });
        }
    }

    fn unpark(&mut self, ix: usize) {
        let p = self.parked.remove(ix);
        self.session = Some(p.session);
        self.objects = p.objects;
        self.tree = p.tree;
    }

    /// Brings an open connection to the front. Pure in-memory swap; the tab strip is
    /// shared and stays as it is.
    pub(super) fn activate(&mut self, sid: SessionId, cx: &mut Context<Self>) {
        if self.active_sid() == Some(sid) {
            return;
        }
        let Some(ix) = self.parked.iter().position(|p| p.session.session_id == sid) else {
            return;
        };
        self.park_active();
        // `park_active` pushed at the end, so `ix` still points at the same entry.
        self.unpark(ix);
        self.status = None;
        cx.notify();
    }

    /// Shows tab `ix` and brings its connection to the front.
    pub(super) fn show_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.tabs.get(ix).map(Tab::ws) else {
            return;
        };
        self.active = ix;
        self.on_home = false;
        self.activate(ws, cx);
        cx.notify();
    }

    /// Takes in a freshly opened session. It comes to the front only when no tab of
    /// another connection is on screen; otherwise it waits parked, so connecting never
    /// changes what the main area shows.
    pub(super) fn adopt(&mut self, info: SessionInfo, objects: Vec<SchemaObject>) {
        let showing_tab = !self.on_home && !self.tabs.is_empty();
        if showing_tab && self.session.is_some() {
            self.parked.push(Parked {
                session: info,
                objects,
                tree: ObjectTree::default(),
            });
            return;
        }
        self.park_active();
        self.session = Some(info);
        self.objects = objects;
        self.tree = ObjectTree::default();
    }

    /// Closes one session and its tabs. If its tab was on screen, the nearest other tab
    /// (or Home) takes its place.
    pub(super) fn close_session(&mut self, sid: SessionId, cx: &mut Context<Self>) {
        // Sessions this connection opened on other databases close with it.
        let mut closing = vec![sid];
        if let Some(t) = self.tree_of(sid) {
            closing.extend(t.extra_sessions());
        }
        let shown = self.tabs.get(self.active).map(Tab::ws);
        // Drop this connection's tabs, keeping the one on screen selected if it stays.
        let kept_active = self
            .tabs
            .iter()
            .take(self.active)
            .filter(|t| t.ws() != sid)
            .count();
        self.tabs.retain(|t| t.ws() != sid);
        self.active = kept_active.min(self.tabs.len().saturating_sub(1));
        if self.active_sid() == Some(sid) {
            self.session = None;
            self.objects.clear();
            self.tree = ObjectTree::default();
            if !self.parked.is_empty() {
                self.unpark(self.parked.len() - 1);
            }
        } else {
            self.parked.retain(|p| p.session.session_id != sid);
        }
        // Keep the front connection matching the tab on screen.
        if shown == Some(sid) || !self.on_home {
            if let Some(ws) = self.tabs.get(self.active).map(Tab::ws) {
                self.activate(ws, cx);
            }
        }
        let engine = self.engine.clone();
        // Closing a network session can wait on the server; keep it off the UI thread.
        cx.background_spawn(async move {
            for id in closing {
                engine.close(id).ok();
            }
        })
        .detach();
        self.status = None;
        cx.notify();
    }

    /// Closes every open connection and its tabs; the window shows Home.
    pub(super) fn disconnect_all(&mut self, cx: &mut Context<Self>) {
        let all: Vec<SessionId> = self
            .session
            .iter()
            .chain(self.parked.iter().map(|p| &p.session))
            .map(|s| s.session_id)
            .collect();
        for sid in all {
            self.close_session(sid, cx);
        }
        self.on_home = true;
        cx.notify();
    }

    /// Tab `ix`, if it still belongs to connection `ws` (a late answer for a closed or
    /// moved tab is dropped).
    pub(super) fn tab_at(&mut self, ws: SessionId, ix: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(ix).filter(|t| t.ws() == ws)
    }

    /// The shared tab strip, while connection `sid` is open.
    pub(super) fn tabs_of(&mut self, sid: SessionId) -> Option<&mut Vec<Tab>> {
        self.is_live(sid).then_some(&mut self.tabs)
    }

    /// The sidebar tree of session `sid`, in front or parked.
    pub(super) fn tree_of(&mut self, sid: SessionId) -> Option<&mut ObjectTree> {
        if self.active_sid() == Some(sid) {
            return Some(&mut self.tree);
        }
        self.parked
            .iter_mut()
            .find(|p| p.session.session_id == sid)
            .map(|p| &mut p.tree)
    }
}
