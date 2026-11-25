//! Home screen (shown while no session is open) and the two-step New connection dialog.
//! See docs/spec/home.md. A child module of `app`, so it can read the app's view state.

use super::*;

/// Steps of the New connection dialog.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ConnDialog {
    /// Pick a database type from the grid.
    Pick,
    /// Fill in the chosen driver's fields.
    Form,
}

pub(super) struct HomeState {
    /// Filters the saved-connection list in the home sidebar.
    pub filter: Entity<InputState>,
    /// Filters the database types in the dialog's first step.
    pub driver_search: Entity<InputState>,
    pub dialog: Option<ConnDialog>,
    /// Connection groups in display order, cached from the engine (render never asks it).
    pub folders: Vec<String>,
    /// Groups the user folded shut in the sidebar (view state only).
    pub collapsed: std::collections::HashSet<String>,
    /// Name box of the New group dialog.
    pub group_name: Entity<InputState>,
    /// Group picker in the New connection form; item 0 is "No group".
    pub group_pick: Entity<SelectState<Vec<SharedString>>>,
}

const NO_GROUP: &str = "No group";

impl HomeState {
    pub fn new(window: &mut Window, cx: &mut Context<ThaanApp>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let driver_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search database types"));
        for input in [&filter, &driver_search] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        let group_name = cx.new(|cx| InputState::new(window, cx).placeholder("Group name"));
        cx.subscribe_in(
            &group_name,
            window,
            |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.create_group(window, cx);
                }
            },
        )
        .detach();
        let group_pick = cx.new(|cx| {
            SelectState::new(
                vec![SharedString::from(NO_GROUP)],
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        Self {
            filter,
            driver_search,
            dialog: None,
            folders: Vec::new(),
            collapsed: Default::default(),
            group_name,
            group_pick,
        }
    }
}

/// What a dragged connection carries; it also draws itself under the pointer.
#[derive(Clone)]
struct DraggedConn {
    id: String,
    name: SharedString,
    driver: String,
}

impl Render for DraggedConn {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .h(px(ROW_H))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_sm()
            .child(
                db_icon(&self.driver)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(self.name.clone())
    }
}

/// A menu row in the danger colour, for actions that remove something.
fn danger_item(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        h_flex()
            .gap_2()
            .text_color(cx.theme().danger)
            .child(icon("trash").size_4())
            .child(label)
    })
}

/// The right-click menu of a saved connection.
fn conn_menu(
    menu: PopupMenu,
    this: &Entity<ThaanApp>,
    c: &ConnectionConfig,
    folders: &[String],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let on = |f: fn(&mut ThaanApp, &ConnectionConfig, &mut Window, &mut Context<ThaanApp>)| {
        let this = this.clone();
        let c = c.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            this.update(cx, |app, cx| f(app, &c, window, cx))
        }
    };
    let (copy_name, copy_target) = (c.name.clone(), conn_target(c));
    let groups: Vec<String> = folders.to_vec();
    let current = c.folder.clone();
    let move_this = this.clone();
    let move_id = c.id.clone();
    menu.item(
        PopupMenuItem::new("Connect")
            .icon(icon("plug"))
            .on_click(on(|app, c, window, cx| app.connect(c.clone(), window, cx))),
    )
    .item(
        PopupMenuItem::new("Duplicate")
            .icon(icon("copy"))
            .on_click(on(|app, c, _, cx| app.duplicate_connection(c.clone(), cx))),
    )
    .submenu_with_icon(
        Some(icon("clipboard")),
        "Copy",
        window,
        cx,
        move |m, _, _| {
            let (n, t) = (copy_name.clone(), copy_target.clone());
            m.item(PopupMenuItem::new("Name").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(n.clone()))
            }))
            .item(
                PopupMenuItem::new("File or host").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(t.clone()))
                }),
            )
        },
    )
    .separator()
    .submenu_with_icon(
        Some(icon("folder-input")),
        "Move to group",
        window,
        cx,
        move |mut m, _, _| {
            for g in &groups {
                let (this, id, g2) = (move_this.clone(), move_id.clone(), g.clone());
                let here = current.as_deref() == Some(g.as_str());
                m = m.item(
                    PopupMenuItem::new(g.clone())
                        .icon(icon("folder"))
                        .disabled(here)
                        .on_click(move |_, _, cx| {
                            this.update(cx, |app, cx| {
                                app.move_to_group(id.clone(), Some(g2.clone()), cx)
                            })
                        }),
                );
            }
            if !groups.is_empty() {
                m = m.separator();
            }
            let (this, id) = (move_this.clone(), move_id.clone());
            m.item(
                PopupMenuItem::new("No group")
                    .disabled(current.is_none())
                    .on_click(move |_, _, cx| {
                        this.update(cx, |app, cx| app.move_to_group(id.clone(), None, cx))
                    }),
            )
        },
    )
    .item(
        PopupMenuItem::new("New group…")
            .icon(icon("folder-plus"))
            .on_click(on(|app, _, window, cx| app.open_group_dialog(window, cx))),
    )
    .separator()
    .item(
        danger_item("Delete connection…").on_click(on(|app, c, window, cx| {
            app.confirm_delete_connection(c.id.clone(), c.name.clone(), window, cx)
        })),
    )
}

/// Database families, used to group the driver grid. Our own grouping.
fn family(driver_id: &str) -> &'static str {
    match driver_id {
        "mongodb" => "Document",
        "sqlite" => "Embedded",
        _ => "Relational",
    }
}

fn driver_note(driver_id: &str) -> &'static str {
    match driver_id {
        "sqlite" => "A single file on this computer",
        "postgres" => "PostgreSQL and compatible servers",
        "mysql" => "MySQL and MariaDB servers",
        "mongodb" => "Collections and documents",
        _ => "",
    }
}

/// One line describing where a saved connection points: file, or host and database.
fn conn_target(c: &ConnectionConfig) -> String {
    if let Some(p) = c.param("path") {
        return p.to_string();
    }
    let host = c.param("host").unwrap_or("");
    let port = c.param("port").map(|p| format!(":{p}")).unwrap_or_default();
    match c.param("database") {
        Some(db) if !db.is_empty() => format!("{host}{port}/{db}"),
        _ => format!("{host}{port}"),
    }
}

impl ThaanApp {
    /// Opens the New connection dialog in the kit's dialog layer. The builder reads this
    /// view's state on every frame, so step changes and typing show up without reopening.
    pub(super) fn open_conn_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.home.dialog = Some(ConnDialog::Pick);
        let mut items = vec![SharedString::from(NO_GROUP)];
        items.extend(self.home.folders.iter().cloned().map(SharedString::from));
        self.home.group_pick.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_index(Some(IndexPath::new(0)), window, cx);
        });
        self.home.driver_search.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let parts = this.update(cx, |app, cx| app.conn_dialog_parts(cx));
            let this = this.clone();
            let dialog = dialog.w(px(760.)).on_close(move |_, _, cx| {
                this.update(cx, |app, cx| {
                    app.home.dialog = None;
                    cx.notify();
                });
            });
            match parts {
                Some((title, body, footer)) => dialog.title(title).child(body).footer(footer),
                None => dialog,
            }
        });
        cx.notify();
    }

    /// Asks before deleting a saved connection. Only the saved entry goes; the database is
    /// not touched, and the dialog says so.
    pub(super) fn confirm_delete_connection(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = id.clone();
            alert
                .confirm()
                .title(SharedString::from(format!("Delete “{name}”?")))
                .description(
                    "This removes the saved connection from ThaanDB. \
                     The database and its data are not changed.",
                )
                .ok_text("Delete")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, _, cx| {
                    this.update(cx, |app, cx| app.delete_connection(id.clone(), cx));
                    true
                })
        });
    }

    /// Deletes a saved connection off the UI thread (it rewrites the store on disk).
    fn delete_connection(&mut self, id: String, cx: &mut Context<Self>) {
        // Drop it from the list at once so the UI answers immediately.
        self.connections.retain(|c| c.id != id);
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine.delete_connection(&id).map(|_| engine.connections())
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(list) => this.connections = list,
                Err(e) => this.set_status(format!("Delete failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Opens the New group dialog (a name box and Create).
    pub(super) fn open_group_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.home.group_name.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let input = this.read(cx).home.group_name.clone();
            let create = this.clone();
            dialog
                .w(px(400.))
                .title("New group")
                .child(
                    v_flex().gap_2().child(Input::new(&input)).child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Right-click a connection to move it into a group"),
                    ),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("group-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("group-create")
                                .primary()
                                .label("Create")
                                .on_click(move |_, window, cx| {
                                    create.update(cx, |app, cx| app.create_group(window, cx))
                                }),
                        ),
                )
        });
    }

    /// Creates the group typed in the dialog. Checks the name here so the dialog can stay
    /// open on a mistake, then saves in the background.
    fn create_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.home.group_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        if self
            .home
            .folders
            .iter()
            .any(|f| f.to_lowercase() == name.to_lowercase())
        {
            self.set_status(format!("A group named “{name}” exists"), true, cx);
            return;
        }
        window.close_dialog(cx);
        self.home.folders.push(name.clone());
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.create_folder(&name) })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(list) => {
                    this.home.folders = list;
                    cx.notify();
                }
                Err(e) => this.set_status(format!("Group not created: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Asks before deleting a group; its connections stay, outside any group.
    fn confirm_delete_group(
        &mut self,
        name: String,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let name = name.clone();
            alert
                .confirm()
                .title(SharedString::from(format!("Delete group “{name}”?")))
                .description(SharedString::from(match count {
                    0 => "The group is empty.".to_string(),
                    1 => "Its connection stays, outside any group.".to_string(),
                    n => format!("Its {n} connections stay, outside any group."),
                }))
                .ok_text("Delete group")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, _, cx| {
                    let name = name.clone();
                    this.update(cx, |app, cx| app.delete_group(name, cx));
                    true
                })
        });
    }

    fn delete_group(&mut self, name: String, cx: &mut Context<Self>) {
        self.home.folders.retain(|f| *f != name);
        for c in &mut self.connections {
            if c.folder.as_deref() == Some(name.as_str()) {
                c.folder = None;
            }
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine
                        .delete_folder(&name)
                        .map(|list| (list, engine.connections()))
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok((folders, conns)) => {
                    this.home.folders = folders;
                    this.connections = conns;
                    cx.notify();
                }
                Err(e) => this.set_status(format!("Group not deleted: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Moves a connection into a group, or out of its group with `None`.
    fn move_to_group(&mut self, id: String, folder: Option<String>, cx: &mut Context<Self>) {
        if let Some(c) = self.connections.iter_mut().find(|c| c.id == id) {
            c.folder = folder.clone();
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine
                        .move_connection(&id, folder.as_deref())
                        .map(|_| (engine.folders(), engine.connections()))
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok((folders, conns)) => {
                    this.home.folders = folders;
                    this.connections = conns;
                    cx.notify();
                }
                Err(e) => this.set_status(format!("Move failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// The group chosen in the New connection form, if any.
    fn picked_group(&self, cx: &App) -> Option<String> {
        self.home
            .group_pick
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .filter(|s| s != NO_GROUP)
    }

    /// Drag and drop: puts a connection in `folder`, just above `before` (or last), right
    /// away in the view and then in the store, in the background.
    fn place_connection(
        &mut self,
        id: String,
        folder: Option<String>,
        before: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if before.as_deref() == Some(id.as_str()) {
            return;
        }
        if let Some(from) = self.connections.iter().position(|c| c.id == id) {
            let mut c = self.connections.remove(from);
            c.folder = folder.clone();
            let to = before
                .as_deref()
                .and_then(|b| self.connections.iter().position(|x| x.id == b))
                .unwrap_or(self.connections.len());
            self.connections.insert(to, c);
        }
        if let Some(f) = &folder {
            if !self.home.folders.contains(f) {
                self.home.folders.push(f.clone());
            }
            self.home.collapsed.remove(f);
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine
                        .place_connection(&id, folder.as_deref(), before.as_deref())
                        .map(|_| (engine.folders(), engine.connections()))
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok((folders, conns)) => {
                    this.home.folders = folders;
                    this.connections = conns;
                    cx.notify();
                }
                Err(e) => this.set_status(format!("Move failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Saves a copy of a connection, named "… copy", right after it in the same group.
    fn duplicate_connection(&mut self, c: ConnectionConfig, cx: &mut Context<Self>) {
        let mut copy = c.clone();
        copy.id = String::new();
        copy.name = format!("{} copy", c.name);
        let engine = self.engine.clone();
        let after = c.id.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let saved = engine.save_connection(copy)?;
                    // Put the copy right below the original.
                    let conns = engine.connections();
                    let next = conns
                        .iter()
                        .position(|x| x.id == after)
                        .and_then(|i| conns.get(i + 1))
                        .map(|x| x.id.clone())
                        .filter(|n| *n != saved.id);
                    engine.place_connection(&saved.id, saved.folder.as_deref(), next.as_deref())?;
                    Ok::<_, thaan_core::Error>((saved.name, engine.connections()))
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok((name, conns)) => {
                    this.connections = conns;
                    this.set_status(format!("Saved “{name}”"), false, cx);
                }
                Err(e) => this.set_status(format!("Duplicate failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// One saved connection in the home sidebar. `indent` puts it under a group header.
    /// Drag it to reorder or regroup; drop on it to put the dragged one above it.
    fn home_conn_row(
        &self,
        i: usize,
        c: &ConnectionConfig,
        indent: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let color = c
            .color
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(theme.primary);
        let id = c.id.clone();
        let name = c.name.clone();
        let this = cx.entity();
        let folders = self.home.folders.clone();
        let conn = c.clone();
        let menu_conn = c.clone();
        let drop_folder = c.folder.clone();
        let drop_before = c.id.clone();
        let accent = theme.primary;
        h_flex()
            .id(("home-conn", i))
            .flex_none()
            .h(px(ROW_H + 4.))
            .gap_2()
            .pr_2()
            .pl(px(if indent { 28. } else { 8. }))
            .rounded(px(6.))
            // A 2px line above the row shows where a dragged connection will land.
            .border_t_2()
            .border_color(transparent_black())
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(theme.sidebar_accent))
            .child(
                db_icon(&c.driver)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(div().flex_1().truncate().child(c.name.clone()))
            .child(conn_dot(color))
            .child(
                Button::new(("home-del", i))
                    .ghost()
                    .xsmall()
                    .icon(icon("trash"))
                    .tooltip("Delete connection")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.confirm_delete_connection(id.clone(), name.clone(), window, cx);
                    })),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.connect(conn.clone(), window, cx)),
            )
            .on_drag(
                DraggedConn {
                    id: c.id.clone(),
                    name: c.name.clone().into(),
                    driver: c.driver.clone(),
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .drag_over::<DraggedConn>(move |s, _, _, _| s.border_color(accent))
            .on_drop(cx.listener(move |this, d: &DraggedConn, _, cx| {
                this.place_connection(
                    d.id.clone(),
                    drop_folder.clone(),
                    Some(drop_before.clone()),
                    cx,
                )
            }))
            .context_menu(move |menu, window, cx| {
                conn_menu(menu, &this, &menu_conn, &folders, window, cx)
            })
            .into_any_element()
    }

    /// A group header: chevron, folder, name and count. Click folds it; trash deletes it;
    /// dropping a connection on it moves the connection into the group.
    fn home_group_row(
        &self,
        gi: usize,
        name: &str,
        count: usize,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let toggle = name.to_string();
        let del = name.to_string();
        let drop_into = name.to_string();
        let menu_name = name.to_string();
        let this = cx.entity();
        let hover = theme.sidebar_accent;
        h_flex()
            .id(("home-group", gi))
            .flex_none()
            .h(px(ROW_H + 4.))
            .gap_2()
            .px_2()
            .rounded(px(6.))
            .border_t_2()
            .border_color(transparent_black())
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(theme.sidebar_accent))
            .child(
                icon(if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                })
                .size_3p5()
                .text_color(theme.muted_foreground),
            )
            .child(
                icon(if open { "folder-open" } else { "folder" })
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .font_medium()
                    .child(name.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
            .child(
                Button::new(("home-group-del", gi))
                    .ghost()
                    .xsmall()
                    .icon(icon("trash"))
                    .tooltip("Delete group")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.confirm_delete_group(del.clone(), count, window, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.home.collapsed.remove(&toggle) {
                    this.home.collapsed.insert(toggle.clone());
                }
                cx.notify();
            }))
            .drag_over::<DraggedConn>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, d: &DraggedConn, _, cx| {
                this.place_connection(d.id.clone(), Some(drop_into.clone()), None, cx)
            }))
            .context_menu(move |menu, _, _| {
                let new_group = this.clone();
                let delete = this.clone();
                let name = menu_name.clone();
                menu.item(
                    PopupMenuItem::new("New group…")
                        .icon(icon("folder-plus"))
                        .on_click(move |_, window, cx| {
                            new_group.update(cx, |app, cx| app.open_group_dialog(window, cx))
                        }),
                )
                .separator()
                .item(danger_item("Delete group…").on_click(move |_, window, cx| {
                    let name = name.clone();
                    delete.update(cx, |app, cx| {
                        app.confirm_delete_group(name, count, window, cx)
                    })
                }))
            })
            .into_any_element()
    }

    /// The home screen: saved connections on the left, an overview on the right.
    pub(super) fn render_home(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let q = self.home.filter.read(cx).value().to_lowercase();
        let saved = self.connections.clone();
        let history = self.engine.history(200);
        let saved_queries = self.engine.saved_queries().len();

        // ---- sidebar: groups with their connections, then connections in no group ----
        let matches = |c: &ConnectionConfig| q.is_empty() || c.name.to_lowercase().contains(&q);
        let mut rows: Vec<AnyElement> = Vec::new();
        for (gi, g) in self.home.folders.clone().iter().enumerate() {
            let members: Vec<(usize, &ConnectionConfig)> = saved
                .iter()
                .enumerate()
                .filter(|(_, c)| c.folder.as_deref() == Some(g.as_str()))
                .collect();
            let name_hit = !q.is_empty() && g.to_lowercase().contains(&q);
            let shown: Vec<_> = members
                .iter()
                .filter(|(_, c)| name_hit || matches(c))
                .collect();
            if !q.is_empty() && shown.is_empty() && !name_hit {
                continue;
            }
            // A search opens every group so matches are never hidden.
            let open = !q.is_empty() || !self.home.collapsed.contains(g);
            rows.push(self.home_group_row(gi, g, members.len(), open, cx));
            if open {
                for (i, c) in shown {
                    rows.push(self.home_conn_row(*i, c, true, cx));
                }
            }
        }
        for (i, c) in saved.iter().enumerate() {
            let grouped = c
                .folder
                .as_ref()
                .is_some_and(|f| self.home.folders.contains(f));
            if !grouped && matches(c) {
                rows.push(self.home_conn_row(i, c, false, cx));
            }
        }
        let sidebar =
            v_flex()
                .w(px(280.))
                .flex_none()
                .h_full()
                .bg(theme.sidebar)
                .border_r_1()
                .border_color(theme.sidebar_border)
                .child(
                    h_flex()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .gap_1()
                        .child(div().flex_1().child(Input::new(&self.home.filter).small()))
                        .child(
                            Button::new("home-new-group")
                                .ghost()
                                .xsmall()
                                .icon(icon("folder-plus"))
                                .tooltip("New group")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_group_dialog(window, cx)
                                })),
                        )
                        .child(
                            Button::new("home-new")
                                .ghost()
                                .xsmall()
                                .icon(icon("plus"))
                                .tooltip("New connection")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_conn_dialog(window, cx)
                                })),
                        ),
                )
                .child(
                    group_label("Connections", Some(saved.len()), &theme)
                        .px_4()
                        .pt_1()
                        .pb_1(),
                )
                .child(
                    v_flex()
                        .id("home-conns")
                        .flex_1()
                        .overflow_y_scroll()
                        .px_2()
                        .pb_2()
                        .gap_0p5()
                        .when(saved.is_empty(), |d| {
                            d.child(
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("No saved connections yet"),
                            )
                        })
                        .children(rows)
                        .child({
                            let accent = theme.primary;
                            div()
                                .id("home-drop-end")
                                .flex_1()
                                .min_h(px(40.))
                                .border_t_2()
                                .border_color(transparent_black())
                                .drag_over::<DraggedConn>(move |s, _, _, _| s.border_color(accent))
                                .on_drop(cx.listener(|this, d: &DraggedConn, _, cx| {
                                    this.place_connection(d.id.clone(), None, None, cx)
                                }))
                        }),
                );

        // ---- overview ----
        let card = || {
            v_flex()
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
        };
        let card_head = |title: &'static str, ic: &'static str| {
            h_flex()
                .h(px(48.))
                .px_5()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(icon(ic).size_4().text_color(theme.muted_foreground))
                .child(div().text_sm().font_medium().child(title))
        };
        let stat = |label: &'static str, value: usize, ic: &'static str| {
            card()
                .flex_1()
                .px_5()
                .py_4()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(icon(ic).size_3p5())
                        .child(label),
                )
                .child(
                    div()
                        .text_2xl()
                        .font_medium()
                        .font_family(mono.clone())
                        .child(value.to_string()),
                )
        };

        let stats = h_flex()
            .gap_4()
            .child(stat("Saved connections", saved.len(), "database"))
            .child(stat("Database types", self.drivers.len(), "layers"))
            .child(stat("Saved queries", saved_queries, "bookmark"))
            .child(stat("Queries run", history.len(), "clock"));

        let quick =
            card()
                .flex_1()
                .min_w_0()
                .child(card_head("Quick connect", "plug"))
                .when(saved.is_empty(), |d| {
                    d.child(
                        v_flex()
                            .p_5()
                            .gap_3()
                            .items_start()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Add a connection to see it here")
                            .child(
                                Button::new("quick-new")
                                    .primary()
                                    .small()
                                    .icon(icon("plus"))
                                    .label("New connection")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_conn_dialog(window, cx)
                                    })),
                            ),
                    )
                })
                .children(saved.iter().take(5).enumerate().map(|(i, c)| {
                    let c2 = c.clone();
                    let color = c
                        .color
                        .as_deref()
                        .and_then(parse_hex)
                        .unwrap_or(theme.primary);
                    let driver_name = self
                        .drivers
                        .iter()
                        .find(|d| d.id == c.driver)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| c.driver.clone());
                    h_flex()
                        .id(("quick", i))
                        .px_5()
                        .py_3()
                        .gap_3()
                        .when(i > 0, |d| d.border_t_1().border_color(theme.border))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(
                            db_icon(&c.driver)
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(div().text_sm().font_medium().child(c.name.clone()))
                                        .child(conn_dot(color)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{driver_name} · {}", conn_target(c))),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Connect"),
                        )
                        .child(
                            icon("chevron-right")
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.connect(c2.clone(), window, cx)
                        }))
                }));

        let shortcut = |id: &'static str, ic: &'static str, label: &'static str| {
            h_flex()
                .id(id)
                .h(px(36.))
                .px_3()
                .gap_3()
                .rounded(px(8.))
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(icon(ic).size_4().text_color(theme.muted_foreground))
                .child(div().flex_1().child(label))
        };
        let shortcuts = card()
            .w(px(320.))
            .flex_none()
            .child(card_head("Shortcuts", "keyboard"))
            .child(
                v_flex()
                    .p_2()
                    .gap_0p5()
                    .child(shortcut("sc-new", "plus", "New connection").on_click(
                        cx.listener(|this, _, window, cx| this.open_conn_dialog(window, cx)),
                    ))
                    .child(
                        shortcut("sc-palette", "command", "Command palette")
                            .children(kbd(PALETTE_KEY))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx)
                            })),
                    )
                    .child(
                        shortcut("sc-theme", "sun-moon", "Light or dark theme")
                            .on_click(cx.listener(|_, _, window, cx| theme::toggle(window, cx))),
                    ),
            )
            .child(
                div()
                    .mx_3()
                    .mb_3()
                    .mt_1()
                    .p_3()
                    .rounded(px(8.))
                    .bg(theme.muted.opacity(0.4))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Click a connection to open it. Its tables and views appear in the sidebar."),
            );

        let recent =
            card()
                .child(card_head("Recent queries", "clock"))
                .when(history.is_empty(), |d| {
                    d.child(
                        div()
                            .p_5()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Queries you run appear here"),
                    )
                })
                .children(history.iter().take(5).enumerate().map(|(i, h)| {
                    h_flex()
                        .px_5()
                        .h(px(40.))
                        .gap_3()
                        .when(i > 0, |d| d.border_t_1().border_color(theme.border))
                        .child(div().size(px(6.)).flex_none().rounded_full().bg(
                            if h.error.is_some() {
                                theme.danger
                            } else {
                                theme.muted_foreground.opacity(0.5)
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_family(mono.clone())
                                .child(first_line(&h.sql)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family(mono.clone())
                                .text_color(theme.muted_foreground)
                                .child(if h.error.is_some() {
                                    "Failed".to_string()
                                } else {
                                    format!(
                                        "{} · {} ms",
                                        plural(h.rows.unwrap_or(0), "row"),
                                        h.elapsed_ms
                                    )
                                }),
                        )
                }));

        let headless = card()
            .child(card_head("From the terminal", "terminal"))
            .child(
                v_flex()
                    .p_5()
                    .gap_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(
                        "Every action here is also a command in the thaan CLI, so scripts and coding agents can use the same connections.",
                    )
                    .child(
                        h_flex().gap_2().children(
                            ["thaan commands", "thaan query <url> \"select 1\""].map(|c| {
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.background)
                                    .text_xs()
                                    .font_family(mono.clone())
                                    .text_color(theme.foreground)
                                    .child(c)
                            }),
                        ),
                    ),
            );

        let overview = v_flex()
            .id("home-overview")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(1120.))
                    .px_8()
                    .py_8()
                    .gap_6()
                    .child(stats)
                    .child(h_flex().gap_6().items_start().child(quick).child(shortcuts))
                    .child(recent)
                    .child(headless),
            );

        h_flex()
            .size_full()
            .bg(theme.background)
            .child(sidebar)
            .child(overview)
            .into_any_element()
    }

    /// Title, body and footer of the New connection dialog for its current step.
    fn conn_dialog_parts(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, AnyElement, AnyElement)> {
        let step = self.home.dialog?;
        let theme = cx.theme().clone();

        let title = match step {
            ConnDialog::Pick => "New connection".to_string(),
            ConnDialog::Form => format!("New {} connection", self.driver.name),
        };
        let header = h_flex()
            .gap_2()
            .when(step == ConnDialog::Form, |d| {
                d.child(
                    Button::new("dlg-back")
                        .ghost()
                        .xsmall()
                        .icon(icon("arrow-left"))
                        .tooltip("Choose another database type")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.home.dialog = Some(ConnDialog::Pick);
                            cx.notify();
                        })),
                )
            })
            .child(title);

        let body: AnyElement = match step {
            ConnDialog::Pick => self.render_driver_grid(cx),
            ConnDialog::Form => self.render_conn_form(cx),
        };

        let footer = h_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(db_icon(&self.driver.id).size_4())
                    .child(format!("Selected: {}", self.driver.name)),
            )
            .when(step == ConnDialog::Pick, |d| {
                d.child(
                    Button::new("dlg-next")
                        .primary()
                        .label("Next")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.home.dialog = Some(ConnDialog::Form);
                            cx.notify();
                        })),
                )
            })
            .when(step == ConnDialog::Form, |d| {
                d.child(
                    Button::new("test")
                        .label("Test")
                        .disabled(self.connecting)
                        .on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))),
                )
                .child(Button::new("save").label("Save").on_click(cx.listener(
                    |this, _, window, cx| {
                        match this
                            .form_config(cx)
                            .map_err(|e| e.to_string())
                            .map(|mut c| {
                                c.folder = this.picked_group(cx);
                                c
                            })
                            .and_then(|c| this.engine.save_connection(c).map_err(|e| e.to_string()))
                        {
                            Ok(c) => {
                                this.connections = this.engine.connections();
                                window.close_dialog(cx);
                                this.set_status(format!("Saved “{}”", c.name), false, cx);
                            }
                            Err(e) => this.set_status(e, true, cx),
                        }
                    },
                )))
                .child(
                    Button::new("connect")
                        .primary()
                        .label(if self.connecting {
                            "Connecting…"
                        } else {
                            "Connect"
                        })
                        .disabled(self.connecting)
                        .on_click(
                            cx.listener(|this, _, window, cx| match this.form_config(cx) {
                                Ok(c) => {
                                    window.close_dialog(cx);
                                    this.connect(c, window, cx)
                                }
                                Err(e) => this.set_status(e, true, cx),
                            }),
                        ),
                )
            });

        Some((
            header.into_any_element(),
            div().h(px(420.)).child(body).into_any_element(),
            footer.into_any_element(),
        ))
    }

    /// Step one: the database types, grouped by family and filtered by the search box.
    fn render_driver_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let q = self.home.driver_search.read(cx).value().to_lowercase();
        let mut families: Vec<&'static str> = Vec::new();
        for d in &self.drivers {
            let f = family(&d.id);
            if !families.contains(&f) {
                families.push(f);
            }
        }
        let tile = |i: usize, d: &DriverInfo, cx: &mut Context<Self>| {
            let selected = d.id == self.driver.id;
            v_flex()
                .id(("driver-tile", i))
                .w(px(152.))
                .h(px(112.))
                .p_3()
                .gap_2()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .border_1()
                .border_color(if selected {
                    theme.primary
                } else {
                    theme.border
                })
                .bg(if selected {
                    theme.list_hover
                } else {
                    theme.background
                })
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .bg(theme.muted.opacity(0.6))
                        .child(db_icon(&d.id).size_5().text_color(theme.foreground)),
                )
                .child(div().text_sm().font_medium().child(d.name.clone()))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    this.select_driver(i, window, cx);
                    // A double click goes straight to the form.
                    if ev.click_count() >= 2 {
                        this.home.dialog = Some(ConnDialog::Form);
                    }
                    cx.notify();
                }))
        };
        let mut groups = v_flex().gap_5();
        for f in families {
            let items: Vec<(usize, DriverInfo)> = self
                .drivers
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, d)| family(&d.id) == f)
                .filter(|(_, d)| {
                    q.is_empty()
                        || d.name.to_lowercase().contains(&q)
                        || driver_note(&d.id).to_lowercase().contains(&q)
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            groups = groups.child(
                v_flex()
                    .gap_2()
                    .child(group_label(f, Some(items.len()), &theme))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_3()
                            .children(items.iter().map(|(i, d)| tile(*i, d, cx))),
                    ),
            );
        }
        v_flex()
            .size_full()
            .child(
                div()
                    .px_6()
                    .pt_4()
                    .pb_2()
                    .child(Input::new(&self.home.driver_search).prefix(icon("search").size_4())),
            )
            .child(
                v_flex()
                    .id("driver-grid")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_6()
                    .py_3()
                    .child(groups)
                    .child(
                        div()
                            .pt_4()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(driver_note(&self.driver.id)),
                    ),
            )
            .into_any_element()
    }

    /// Step two: name, URL and the driver's own fields.
    fn render_conn_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("conn-form")
            .size_full()
            .overflow_y_scroll()
            .px_6()
            .py_5()
            .gap_4()
            .child(field(
                "Name",
                Input::new(&self.conn_name).into_any_element(),
                &theme,
            ))
            .child(field(
                "Connection URL",
                Input::new(&self.conn_url).into_any_element(),
                &theme,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("Or fill in the {} fields", self.driver.name)),
            )
            .children(
                self.driver
                    .params
                    .iter()
                    .zip(&self.conn_params)
                    .map(|(spec, (_, _, st))| {
                        field(&spec.label, Input::new(st).into_any_element(), &theme)
                    }),
            )
            .child(field(
                "Group",
                div()
                    .w(px(240.))
                    .child(Select::new(&self.home.group_pick).small())
                    .into_any_element(),
                &theme,
            ))
            .child(
                Checkbox::new("ro")
                    .label("Read-only, refuse writes")
                    .checked(self.conn_read_only)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        this.conn_read_only = *v;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
