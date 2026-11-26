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
    /// The saved connection the form is editing; Save then updates it in place.
    pub editing: Option<String>,
    /// Last queries run, newest first, with repeats folded (cached; render never asks
    /// the engine). Refreshed by `refresh_home_history`.
    pub recent: Vec<HistoryEntry>,
    /// When each saved connection last ran a query (ms since the epoch), from history.
    pub last_used: std::collections::HashMap<String, u64>,
    /// The database-type picker: the family picked in its left column (`None`: all)
    /// and whether types show as a list instead of tiles (view state only).
    pub driver_family: Option<&'static str>,
    pub driver_list: bool,
}

/// Rows in the Recent queries list.
const RECENT_ROWS: usize = 8;

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
            editing: None,
            recent: Vec::new(),
            last_used: Default::default(),
            driver_family: None,
            driver_list: false,
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

/// A menu row in the danger colour, for actions that remove something. The icon goes
/// through the menu's own icon column, so it lines up with the other items.
fn danger_item(label: &'static str, danger: Hsla) -> PopupMenuItem {
    danger_item_with(label, "trash", danger)
}

pub(super) fn danger_item_with(
    label: &'static str,
    ic: &'static str,
    danger: Hsla,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().text_color(danger).child(label))
        .icon(icon(ic).text_color(danger))
}

/// The right-click menu of a saved connection.
pub(super) fn conn_menu(
    menu: PopupMenu,
    this: &Entity<ThaanApp>,
    c: &ConnectionConfig,
    live: bool,
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
    let groups: Vec<String> = folders.to_vec();
    let current = c.folder.clone();
    let move_this = this.clone();
    let move_id = c.id.clone();
    // Connect stays on the home screen; Open goes to the connection's page.
    let menu = if live {
        menu.item(
            PopupMenuItem::new("Open")
                .icon(icon("arrow-right"))
                .on_click(on(|app, c, window, cx| {
                    app.open_from_home(c.clone(), window, cx)
                })),
        )
        .item(
            PopupMenuItem::new("Disconnect")
                .icon(icon("unplug"))
                .on_click(on(|app, c, _, cx| app.disconnect_from_home(c, cx))),
        )
    } else {
        menu.item(
            PopupMenuItem::new("Connect")
                .icon(icon("plug"))
                .on_click(on(|app, c, window, cx| {
                    app.connect_from_home(c.clone(), window, cx)
                })),
        )
        .item(
            PopupMenuItem::new("Connect and open")
                .icon(icon("arrow-right"))
                .on_click(on(|app, c, window, cx| {
                    app.open_from_home(c.clone(), window, cx)
                })),
        )
    };
    let menu =
        menu.item(PopupMenuItem::new("Edit").icon(icon("pencil")).on_click(on(
            |app, c, window, cx| app.edit_connection(c.clone(), window, cx),
        )))
        .item(
            PopupMenuItem::new("Duplicate")
                .icon(icon("copy"))
                .on_click(on(|app, c, _, cx| app.duplicate_connection(c.clone(), cx))),
        );
    copy_submenu(menu, this, c, window, cx)
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
            PopupMenuItem::new("New group")
                .icon(icon("folder-plus"))
                .on_click(on(|app, _, window, cx| app.open_group_dialog(window, cx))),
        )
        .separator()
        .item(
            danger_item("Delete connection", cx.theme().danger).on_click(on(
                |app, c, window, cx| {
                    app.confirm_delete_connection(c.id.clone(), c.name.clone(), window, cx)
                },
            )),
        )
}

/// The Copy submenu of a connection: name, URL, URL with password, file or host.
pub(super) fn copy_submenu(
    menu: PopupMenu,
    this: &Entity<ThaanApp>,
    c: &ConnectionConfig,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (copy_name, copy_target) = (c.name.clone(), conn_target(c));
    let (copy_this, copy_conn) = (this.clone(), c.clone());
    menu.submenu_with_icon(
        Some(icon("clipboard")),
        "Copy",
        window,
        cx,
        move |m, _, _| {
            let (n, t) = (copy_name.clone(), copy_target.clone());
            let (a1, c1) = (copy_this.clone(), copy_conn.clone());
            let (a2, c2) = (copy_this.clone(), copy_conn.clone());
            let has_url = copy_conn.to_url(None).is_some();
            // SQLite files have no password; the plain URL is the whole story.
            let has_password = has_url && copy_conn.driver != "sqlite";
            m.item(
                PopupMenuItem::new("Name")
                    .icon(icon("copy"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(n.clone()))
                    }),
            )
            .item(
                PopupMenuItem::new("Connection URL")
                    .icon(icon("link"))
                    .disabled(!has_url)
                    .on_click(move |_, _, cx| {
                        a1.update(cx, |app, cx| app.copy_connection_url(&c1, false, cx))
                    }),
            )
            .item(
                PopupMenuItem::new("Connection URL with password")
                    .icon(icon("link"))
                    .disabled(!has_password)
                    .on_click(move |_, _, cx| {
                        a2.update(cx, |app, cx| app.copy_connection_url(&c2, true, cx))
                    }),
            )
            .item(
                PopupMenuItem::new("File or host")
                    .icon(icon("server"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(t.clone()))
                    }),
            )
        },
    )
}

/// Database families, used to group the driver grid. Our own grouping.
fn family(driver_id: &str) -> &'static str {
    match driver_id {
        "mongodb" => "Document",
        "redis" => "Key-value",
        "mqtt" => "Message broker",
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
        "redis" => "Keys, hashes, lists and streams",
        "mqtt" => "Topics and live messages",
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
    /// Copies a saved connection as a URL. With the password, it comes from this run's
    /// memory or the keychain, read off the UI thread (the keychain can be slow).
    pub(super) fn copy_connection_url(
        &mut self,
        c: &ConnectionConfig,
        with_password: bool,
        cx: &mut Context<Self>,
    ) {
        if !with_password {
            if let Some(url) = c.to_url(None) {
                cx.write_to_clipboard(ClipboardItem::new_string(url));
                self.set_status(format!("Copied the URL of “{}”", c.name), false, cx);
            }
            return;
        }
        let engine = self.engine.clone();
        let c = c.clone();
        cx.spawn(async move |this, cx| {
            let typed = c.param("password").map(str::to_string);
            let id = c.id.clone();
            let password = match typed {
                Some(p) => Some(p),
                None => {
                    cx.background_spawn(async move { engine.secret(&id, "password") })
                        .await
                }
            };
            this.update(cx, |this, cx| {
                let Some(url) = c.to_url(password.as_deref()) else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(url));
                let msg = if password.is_some() {
                    format!("Copied the URL of “{}” with its password", c.name)
                } else {
                    format!("Copied the URL of “{}” (no saved password)", c.name)
                };
                this.set_status(msg, false, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the New connection dialog in the kit's dialog layer. The builder reads this
    /// view's state on every frame, so step changes and typing show up without reopening.
    pub(super) fn open_conn_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.home.dialog = Some(ConnDialog::Pick);
        // The form is shared with Edit: start a new connection from empty fields.
        if self.home.editing.take().is_some() {
            let ix = self
                .drivers
                .iter()
                .position(|d| d.id == self.driver.id)
                .unwrap_or(0);
            self.select_driver(ix, window, cx);
            self.conn_name
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.conn_read_only = false;
            self.conn_color
                .update(cx, |p, cx| p.set_value(rgb(0x14b8a6), window, cx));
        }
        self.conn_url
            .update(cx, |s, cx| s.set_value("", window, cx));
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
        self.show_conn_dialog(window, cx);
    }

    /// Saves the store file and the password (OS keychain) off the UI thread: the
    /// keychain can take a moment or ask the user to allow access.
    fn save_connection_in_background(&mut self, c: ConnectionConfig, cx: &mut Context<Self>) {
        self.set_status(format!("Saving “{}”…", c.name), false, cx);
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    engine
                        .save_connection(c)
                        .map(|c| (c.name, engine.connections()))
                })
                .await;
            this.update(cx, |this, cx| match res {
                Ok((name, conns)) => {
                    this.connections = conns;
                    this.set_status(format!("Saved “{name}”"), false, cx);
                }
                Err(e) => this.set_status(format!("Save failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Opens the connection form filled in from a saved connection. Save updates that
    /// entry (same id, same place in the list); Connect uses the edited settings.
    /// The password comes from the OS keychain, read off the UI thread, and fills the
    /// (masked) box when it arrives.
    pub(super) fn edit_connection(
        &mut self,
        c: ConnectionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.drivers.iter().position(|d| d.id == c.driver) {
            self.select_driver(ix, window, cx);
        }
        self.home.dialog = Some(ConnDialog::Form);
        self.home.editing = Some(c.id.clone());
        self.conn_name
            .update(cx, |s, cx| s.set_value(c.name.clone(), window, cx));
        self.conn_url
            .update(cx, |s, cx| s.set_value("", window, cx));
        for (key, _, st) in &self.conn_params {
            if let Some(v) = c.params.get(key) {
                st.update(cx, |s, cx| s.set_value(v.clone(), window, cx));
            }
        }
        for (key, picker) in &self.conn_choices {
            if let Some(v) = c.params.get(key) {
                let v = SharedString::from(v.clone());
                picker.update(cx, |p, cx| p.set_selected_value(&v, window, cx));
            }
        }
        self.conn_read_only = c.read_only;
        if let Some(color) = c.color.as_deref().and_then(parse_hex) {
            self.conn_color
                .update(cx, |p, cx| p.set_value(color, window, cx));
        }
        let mut items = vec![SharedString::from(NO_GROUP)];
        items.extend(self.home.folders.iter().cloned().map(SharedString::from));
        let group = SharedString::from(c.folder.clone().unwrap_or_else(|| NO_GROUP.into()));
        self.home.group_pick.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&group, window, cx);
        });
        self.show_conn_dialog(window, cx);
        self.fill_stored_secrets(c.id, window, cx);
    }

    /// Fills empty secret boxes of the form (password, SSH passphrase) from the keychain,
    /// if the same connection is still being edited when the answer arrives.
    fn fill_stored_secrets(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<String> = self
            .conn_params
            .iter()
            .filter(|(k, _, st)| {
                thaan_core::store::SECRET_KEYS.contains(&k.as_str())
                    && st.read(cx).value().is_empty()
            })
            .map(|(k, _, _)| k.clone())
            .collect();
        if id.is_empty() || keys.is_empty() {
            return;
        }
        let engine = self.engine.clone();
        let conn = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    keys.into_iter()
                        .filter_map(|k| engine.secret(&conn, &k).map(|v| (k, v)))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.home.editing.as_deref() != Some(id.as_str()) {
                    return;
                }
                for (k, v) in found {
                    if let Some((_, _, st)) = this.conn_params.iter().find(|(key, _, _)| *key == k)
                    {
                        // Only an untouched box: never overwrite what the user typed.
                        if st.read(cx).value().is_empty() {
                            st.update(cx, |s, cx| s.set_value(v, window, cx));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_conn_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let parts = this.update(cx, |app, cx| app.conn_dialog_parts(cx));
            let this = this.clone();
            let dialog = dialog.w(px(840.)).on_close(move |_, _, cx| {
                this.update(cx, |app, cx| {
                    app.home.dialog = None;
                    // `editing` stays set until the next New, which clears the form.
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
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            // The engine copies the password too (from the keychain) and places the copy
            // right below the original.
            let res = cx
                .background_spawn(async move {
                    let saved = engine.duplicate_connection(&c.id, None)?;
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

    /// Connects in the background and stays on the home screen; the row gets a live dot.
    pub(super) fn connect_from_home(
        &mut self,
        c: ConnectionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_home = true;
        if self.live_for(&c.id).is_none() {
            self.connect(c, window, cx);
        }
        cx.notify();
    }

    /// A click on a connection row from home only connects (and stays here); it never
    /// opens the workspace, so a double-click on a row that is already open does nothing.
    /// The row's → button (or right-click → Open) opens it.
    pub(super) fn click_from_home(
        &mut self,
        c: ConnectionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connect_from_home(c, window, cx);
    }

    /// Opens a connection in the workspace, connecting first when it is not open yet.
    pub(super) fn open_from_home(
        &mut self,
        c: ConnectionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // "Open" is the one action that opens a page: a query tab for this connection,
        // right away when it is open, or once the connect finishes.
        if let Some(sid) = self.live_for(&c.id) {
            self.activate(sid, cx);
            self.new_query_tab(None, window, cx);
            return;
        }
        self.open_after_connect = Some(c.id.clone());
        self.connect(c, window, cx);
    }

    /// Closes a connection opened from home, without leaving the home screen.
    pub(super) fn disconnect_from_home(&mut self, c: &ConnectionConfig, cx: &mut Context<Self>) {
        if let Some(sid) = self.live_for(&c.id) {
            self.close_session(sid, cx);
        }
        cx.notify();
    }

    /// Reloads the home screen's history cache from the already-loaded store (an
    /// in-memory read). Called at startup, after a query finishes and on going home.
    pub(super) fn refresh_home_history(&mut self) {
        let all = self.engine.history(200);
        let mut last_used = std::collections::HashMap::new();
        for h in &all {
            let t = last_used.entry(h.connection_id.clone()).or_insert(0);
            *t = (*t).max(h.at_ms);
        }
        let mut recent: Vec<HistoryEntry> = Vec::new();
        for h in all {
            // Only queries that can be opened again: their connection is still saved.
            if !self.connections.iter().any(|c| c.id == h.connection_id) {
                continue;
            }
            // The same statement run again on the same connection is one row.
            if recent
                .iter()
                .any(|r| r.connection_id == h.connection_id && r.sql.trim() == h.sql.trim())
            {
                continue;
            }
            recent.push(h);
            if recent.len() == RECENT_ROWS {
                break;
            }
        }
        self.home.recent = recent;
        self.home.last_used = last_used;
    }

    /// Opens a past query in a new query tab of its connection, connecting first when
    /// the connection is not open.
    fn open_recent_query(&mut self, h: HistoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self
            .connections
            .iter()
            .find(|c| c.id == h.connection_id)
            .cloned()
        else {
            return self.set_status("That connection was deleted", true, cx);
        };
        let q = SavedQuery {
            id: String::new(),
            name: first_line(&h.sql).chars().take(24).collect(),
            folder: None,
            sql: h.sql,
            connection_id: Some(c.id.clone()),
            updated_ms: 0,
        };
        if let Some(sid) = self.live_for(&c.id) {
            self.activate(sid, cx);
            self.new_query_tab(Some(q), window, cx);
            if let Some(Tab::Query(t)) = self.tabs.last_mut() {
                t.saved_id = None;
            }
            return;
        }
        if self.connecting {
            return;
        }
        self.open_after_connect = Some(c.id.clone());
        self.open_query_after_connect = Some(q);
        self.connect(c, window, cx);
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
        let id = c.id.clone();
        let name = c.name.clone();
        let this = cx.entity();
        let folders = self.home.folders.clone();
        let conn = c.clone();
        let menu_conn = c.clone();
        let drop_folder = c.folder.clone();
        let drop_before = c.id.clone();
        let accent = theme.primary;
        let live = self.live_for(&c.id).is_some();
        let connecting = self.connecting_id.as_deref() == Some(c.id.as_str());
        // An open connection unfolds into the same tree as the workspace sidebar.
        let tree = self.home_tree_open(&c.id);
        let children = match tree {
            Some((_, true)) => self.home_tree_rows(&c.id, if indent { 2 } else { 1 }, cx),
            _ => Vec::new(),
        };
        let row = h_flex()
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
            .child(div().id(("home-chev", i)).w(px(14.)).flex_none().when_some(
                tree,
                |d, (sid, open)| {
                    d.child(
                        icon(if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        })
                        .size_3p5()
                        .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_home_tree(sid, cx);
                    }))
                },
            ))
            .child(db_icon(&c.driver).size_4())
            .child(div().flex_1().truncate().child(c.name.clone()))
            .when(connecting, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Connecting…"),
                )
            })
            .when(live, |d| {
                let open = conn.clone();
                d.child(conn_dot(theme.success)).child(
                    Button::new(("home-open", i))
                        .ghost()
                        .xsmall()
                        .icon(icon("arrow-right"))
                        .tooltip("Open in the workspace")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_from_home(open.clone(), window, cx);
                        })),
                )
            })
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
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(if live {
                    "Connected: → opens it"
                } else {
                    "Connect (stays on this page)"
                })
                .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.click_from_home(conn.clone(), window, cx)
            }))
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
                conn_menu(menu, &this, &menu_conn, live, &folders, window, cx)
            });
        if children.is_empty() {
            return row.into_any_element();
        }
        v_flex()
            .gap_0p5()
            .child(row)
            .children(children)
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
            .context_menu(move |menu, _, cx| {
                let danger = cx.theme().danger;
                let new_group = this.clone();
                let delete = this.clone();
                let name = menu_name.clone();
                menu.item(
                    PopupMenuItem::new("New group")
                        .icon(icon("folder-plus"))
                        .on_click(move |_, window, cx| {
                            new_group.update(cx, |app, cx| app.open_group_dialog(window, cx))
                        }),
                )
                .separator()
                .item(
                    danger_item("Delete group", danger).on_click(move |_, window, cx| {
                        let name = name.clone();
                        delete.update(cx, |app, cx| {
                            app.confirm_delete_group(name, count, window, cx)
                        })
                    }),
                )
            })
            .into_any_element()
    }

    /// The home screen: saved connections on the left, an overview on the right.
    /// The home page. `with_sidebar` is false when it sits in the workspace's Home tab,
    /// next to the workspace's own sidebar tree.
    /// `width` is the space the home page gets. Under `COMPACT_HOME` it drops the
    /// secondary columns (last used, connection and timing of recent queries) and the
    /// side padding shrinks, so names, targets and the Connect button always fit.
    pub(super) fn render_home(
        &mut self,
        with_sidebar: bool,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = width < COMPACT_HOME;
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let q = self.home.filter.read(cx).value().to_lowercase();
        let saved = self.connections.clone();

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

        // ---- overview: the connections, then the queries run lately ----
        let now = now_ms();
        let muted = theme.muted_foreground;
        let panel = || {
            v_flex()
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .overflow_hidden()
        };
        let section_head = |title: &'static str, count: Option<usize>| {
            h_flex()
                .h(px(32.))
                .gap_2()
                .items_baseline()
                .child(div().text_base().font_medium().child(title))
                .when_some(count, |d, n| {
                    d.child(
                        div()
                            .text_sm()
                            .font_family(mono.clone())
                            .text_color(muted)
                            .child(n.to_string()),
                    )
                })
        };

        let conn_rows = saved.iter().enumerate().map(|(i, c)| {
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
            let live = self.live_for(&c.id).is_some();
            let connecting = self.connecting_id.as_deref() == Some(c.id.as_str());
            let used = self.home.last_used.get(&c.id).map(|t| ago(now, *t));
            let this = cx.entity();
            let folders = self.home.folders.clone();
            let menu_conn = c.clone();
            let row_conn = c.clone();
            let btn_conn = c.clone();
            let action = if live {
                Button::new(("home-open", i))
                    .small()
                    .outline()
                    .label("Open")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_from_home(btn_conn.clone(), window, cx)
                    }))
            } else {
                Button::new(("home-connect", i))
                    .small()
                    .ghost()
                    .label(if connecting {
                        "Connecting…"
                    } else {
                        "Connect"
                    })
                    .loading(connecting)
                    .disabled(connecting || self.connecting)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.connect_from_home(btn_conn.clone(), window, cx)
                    }))
            };
            h_flex()
                .id(("home-row", i))
                .h(px(60.))
                .pl_4()
                .pr_3()
                .gap_3()
                .when(i > 0, |d| d.border_t_1().border_color(theme.border))
                .hover(|s| s.bg(theme.list_hover))
                .child(
                    div()
                        .size_8()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .child(db_icon(&c.driver).size_4().text_color(muted)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_sm()
                                        .font_medium()
                                        .truncate()
                                        .child(c.name.clone()),
                                )
                                .child(div().flex_none().child(conn_dot(color)))
                                .when_some(c.folder.clone().filter(|_| !compact), |d, f| {
                                    d.child(div().flex_none().text_xs().text_color(muted).child(f))
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(muted)
                                .child(div().flex_none().child(driver_name))
                                .child(div().flex_none().child("·"))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(mono.clone())
                                        .child(conn_target(c)),
                                ),
                        ),
                )
                .when(!compact, |row| {
                    row.child(
                        div()
                            .w(px(96.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .text_xs()
                            .font_family(mono.clone())
                            .text_color(muted)
                            .when(live, |d| {
                                d.child(
                                    h_flex()
                                        .gap_1p5()
                                        .child(conn_dot(theme.success))
                                        .child("open"),
                                )
                            })
                            .when(!live, |d| d.children(used)),
                    )
                })
                // Compact: no fixed column; an open connection keeps its green dot here.
                .when(compact && live, |row| {
                    row.child(div().flex_none().child(conn_dot(theme.success)))
                })
                .child(
                    h_flex()
                        .when(!compact, |d| d.w(px(112.)))
                        .flex_none()
                        .justify_end()
                        .child(action),
                )
                // A double-click connects; nothing on the row opens a page under the
                // cursor (Open is the button, or right-click).
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.click_from_home(row_conn.clone(), window, cx)
                    }
                }))
                .context_menu(move |menu, window, cx| {
                    conn_menu(menu, &this, &menu_conn, live, &folders, window, cx)
                })
        });

        let connections =
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .child(section_head("Connections", Some(saved.len())))
                        .child(div().flex_1())
                        .child(
                            Button::new("home-new")
                                .primary()
                                .small()
                                .icon(icon("plus"))
                                .label("New connection")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_conn_dialog(window, cx)
                                })),
                        ),
                )
                .child(if saved.is_empty() {
                    panel()
                        .p_6()
                        .gap_1()
                        .child(div().text_sm().child("No saved connections"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("Add one to browse its tables and run queries"),
                        )
                        .into_any_element()
                } else {
                    panel().children(conn_rows).into_any_element()
                });

        let names: std::collections::HashMap<&str, &str> = saved
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        let recent: Vec<(&HistoryEntry, &str)> = self
            .home
            .recent
            .iter()
            .filter_map(|h| names.get(h.connection_id.as_str()).map(|n| (h, *n)))
            .collect();
        let has_recent = !recent.is_empty();
        let recent_rows = recent.into_iter().enumerate().map(|(i, (h, conn_name))| {
            let failed = h.error.is_some();
            let entry = h.clone();
            let conn_name = conn_name.to_string();
            h_flex()
                .id(("home-recent", i))
                .h(px(40.))
                .px_4()
                .gap_3()
                .when(i > 0, |d| d.border_t_1().border_color(theme.border))
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(div().size(px(6.)).flex_none().rounded_full().bg(if failed {
                    theme.danger
                } else {
                    muted.opacity(0.5)
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .child(first_line(&h.sql)),
                )
                .when(!compact, |row| {
                    row.child(
                        div()
                            .w(px(120.))
                            .flex_none()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(conn_name),
                    )
                    .child(
                        div()
                            .w(px(120.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .text_xs()
                            .font_family(mono.clone())
                            .text_color(if failed { theme.danger } else { muted })
                            .child(if failed {
                                "failed".to_string()
                            } else {
                                format!(
                                    "{} · {} ms",
                                    plural(h.rows.unwrap_or(0), "row"),
                                    h.elapsed_ms
                                )
                            }),
                    )
                })
                .child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .text_xs()
                        .font_family(mono.clone())
                        .text_color(muted)
                        .child(ago(now, h.at_ms)),
                )
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open in a new query tab")
                        .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_recent_query(entry.clone(), window, cx)
                }))
        });
        let recent = has_recent.then(|| {
            v_flex()
                .gap_3()
                .child(section_head("Recent queries", None))
                .child(panel().children(recent_rows))
        });

        let hints = h_flex()
            .flex_wrap()
            .gap_x_5()
            .gap_y_2()
            .text_xs()
            .text_color(muted)
            .children(
                [
                    (PALETTE_KEY, "Commands"),
                    (NEW_TAB_KEY, "New query tab"),
                    (RUN_KEY, "Run statement"),
                ]
                .map(|(k, label)| h_flex().gap_2().children(kbd(k)).child(label)),
            );

        let overview = v_flex()
            .id("home-overview")
            .flex_1()
            // Without this the page keeps its rows' natural width and runs off the edge.
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(960.))
                    .map(|d| {
                        if compact {
                            d.px_4().pt_6()
                        } else {
                            d.px_8().pt_8()
                        }
                    })
                    .pb_6()
                    .gap_8()
                    .child(connections)
                    .children(recent)
                    .child(hints),
            );

        h_flex()
            .size_full()
            .bg(theme.background)
            .when(with_sidebar, |d| d.child(sidebar))
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
            ConnDialog::Form => match &self.home.editing {
                Some(_) => format!("Edit {} connection", self.driver.name),
                None => format!("New {} connection", self.driver.name),
            },
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
                        let mut c = match this.form_config(cx) {
                            Ok(c) => c,
                            Err(e) => return this.set_status(e, true, cx),
                        };
                        c.folder = this.picked_group(cx);
                        if let Some(id) = &this.home.editing {
                            c.id = id.clone();
                        }
                        window.close_dialog(cx);
                        this.save_connection_in_background(c, cx);
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
                                Ok(mut c) => {
                                    if let Some(id) = &this.home.editing {
                                        c.id = id.clone();
                                    }
                                    // Started from home: connect and stay there.
                                    if this.session.is_none() || this.on_home {
                                        this.on_home = true;
                                    }
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
    /// Step one: database types. A column of families on the left (All first), a
    /// search box and a tiles / list switch on top, the types on the right.
    fn render_driver_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let q = self.home.driver_search.read(cx).value().to_lowercase();
        let picked = self.home.driver_family;
        let list_view = self.home.driver_list;
        let mut families: Vec<&'static str> = Vec::new();
        for d in &self.drivers {
            let f = family(&d.id);
            if !families.contains(&f) {
                families.push(f);
            }
        }
        let matches = |d: &DriverInfo| {
            q.is_empty()
                || d.name.to_lowercase().contains(&q)
                || driver_note(&d.id).to_lowercase().contains(&q)
        };

        // Left: All, then each family with its count; the picked one is a surface step.
        let fam_row = |id: usize,
                       label: &'static str,
                       n: usize,
                       on: bool,
                       value: Option<&'static str>,
                       cx: &mut Context<Self>| {
            h_flex()
                .id(("driver-family", id))
                .h(px(32.))
                .px_3()
                .gap_2()
                .rounded(px(6.))
                .text_sm()
                .cursor_pointer()
                .when(on, |d| d.bg(theme.list_hover).font_medium())
                .when(!on, |d| d.text_color(muted))
                .hover(|s| s.bg(theme.list_hover))
                .child(div().flex_1().child(label))
                .child(
                    div()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(muted)
                        .child(n.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.home.driver_family = value;
                    cx.notify();
                }))
        };
        let mut nav = v_flex().gap_0p5().child(fam_row(
            0,
            "All",
            self.drivers.len(),
            picked.is_none(),
            None,
            cx,
        ));
        for (i, f) in families.iter().enumerate() {
            let n = self.drivers.iter().filter(|d| family(&d.id) == *f).count();
            nav = nav.child(fam_row(i + 1, f, n, picked == Some(*f), Some(*f), cx));
        }

        let pick = |i: usize| {
            move |this: &mut Self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>| {
                this.select_driver(i, window, cx);
                // A double click goes straight to the form.
                if ev.click_count() >= 2 {
                    this.home.dialog = Some(ConnDialog::Form);
                }
                cx.notify();
            }
        };
        let logo = |d: &DriverInfo, size: f32| {
            div()
                .size(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(theme.muted.opacity(0.6))
                .child(db_icon(&d.id).size_5().text_color(theme.foreground))
        };
        let tile = |i: usize, d: &DriverInfo, cx: &mut Context<Self>| {
            let selected = d.id == self.driver.id;
            v_flex()
                .id(("driver-tile", i))
                .w(px(148.))
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
                .child(logo(d, 36.))
                .child(div().text_sm().font_medium().child(d.name.clone()))
                .on_click(cx.listener(pick(i)))
        };
        let row = |i: usize, d: &DriverInfo, cx: &mut Context<Self>| {
            let selected = d.id == self.driver.id;
            h_flex()
                .id(("driver-row", i))
                .h(px(52.))
                .px_3()
                .gap_3()
                .rounded(px(8.))
                .border_1()
                .border_color(if selected {
                    theme.primary
                } else {
                    transparent_black()
                })
                .when(selected, |d| d.bg(theme.list_hover))
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .child(logo(d, 32.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_medium().child(d.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(driver_note(&d.id)),
                        ),
                )
                .on_click(cx.listener(pick(i)))
        };

        // Right: the picked family, or every family as its own group.
        let shown: Vec<&'static str> = match picked {
            Some(f) => vec![f],
            None => families.clone(),
        };
        let mut groups = v_flex().gap_5();
        let mut any = false;
        for f in shown {
            let items: Vec<(usize, DriverInfo)> = self
                .drivers
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, d)| family(&d.id) == f && matches(d))
                .collect();
            if items.is_empty() {
                continue;
            }
            any = true;
            let body = if list_view {
                v_flex()
                    .gap_1()
                    .children(items.iter().map(|(i, d)| row(*i, d, cx)))
                    .into_any_element()
            } else {
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .children(items.iter().map(|(i, d)| tile(*i, d, cx)))
                    .into_any_element()
            };
            groups = groups.child(
                v_flex()
                    .gap_2()
                    // One family picked: its name is already in the column on the left.
                    .when(picked.is_none(), |d| {
                        d.child(group_label(f, Some(items.len()), &theme))
                    })
                    .child(body),
            );
        }
        if !any {
            groups = groups.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No database types match"),
            );
        }

        let view_btn = |id: &'static str,
                        ic: &'static str,
                        tip: &'static str,
                        list: bool,
                        cx: &mut Context<Self>| {
            Button::new(id)
                .small()
                .ghost()
                .icon(icon(ic))
                .tooltip(tip)
                .selected(list_view == list)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.home.driver_list = list;
                    cx.notify();
                }))
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_6()
                    .pt_4()
                    .pb_3()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_0p5()
                            .p_0p5()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .child(view_btn("dv-grid", "layout-grid", "Tiles", false, cx))
                            .child(view_btn("dv-list", "layout-list", "List", true, cx)),
                    )
                    .child(div().flex_1().child(
                        Input::new(&self.home.driver_search).prefix(icon("search").size_4()),
                    )),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .w(px(184.))
                            .flex_none()
                            .h_full()
                            .p_3()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(nav),
                    )
                    .child(
                        v_flex()
                            .id("driver-grid")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .px_5()
                            .py_4()
                            .child(groups)
                            .when(!list_view, |d| {
                                d.child(
                                    div()
                                        .pt_4()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(driver_note(&self.driver.id)),
                                )
                            }),
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
                        let control = match self.conn_choices.get(&spec.key) {
                            // Fixed width: Select stretches to fill its parent.
                            Some(picker) => div()
                                .w(px(240.))
                                .flex_none()
                                .child(Select::new(picker).small())
                                .into_any_element(),
                            None => Input::new(st).into_any_element(),
                        };
                        field(&spec.label, control, &theme)
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
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().child("Colour"))
                    .child(
                        ColorPicker::new(&self.conn_color).featured_colors(
                            [
                                0x14b8a6, 0x22c55e, 0x3b82f6, 0xf59e0b, 0xef4444, 0xec4899,
                                0x64748b,
                            ]
                            .map(|c| rgb(c).into())
                            .to_vec(),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Marks this connection in the list and on the status bar"),
                    ),
            )
            .into_any_element()
    }
}

/// Below this width the home page drops its secondary columns.
pub(super) const COMPACT_HOME: Pixels = px(640.);

/// Milliseconds since the Unix epoch, for "5 min ago" labels.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long ago `at` was, short: `just now`, `5 min ago`, `3 h ago`, `2 d ago`.
fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}
