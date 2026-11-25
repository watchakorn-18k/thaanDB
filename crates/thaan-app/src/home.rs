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
}

impl HomeState {
    pub fn new(window: &mut Window, cx: &mut Context<ThaanApp>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let driver_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search database types"));
        for input in [&filter, &driver_search] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        Self {
            filter,
            driver_search,
            dialog: None,
        }
    }
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

    /// The home screen: saved connections on the left, an overview on the right.
    pub(super) fn render_home(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let q = self.home.filter.read(cx).value().to_lowercase();
        let saved = self.connections.clone();
        let history = self.engine.history(200);
        let saved_queries = self.engine.saved_queries().len();

        // ---- sidebar: every saved connection ----
        let shown: Vec<(usize, ConnectionConfig)> = saved
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q))
            .collect();
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
                        .children(shown.into_iter().map(|(i, c)| {
                            let color = c
                                .color
                                .as_deref()
                                .and_then(parse_hex)
                                .unwrap_or(theme.primary);
                            let id = c.id.clone();
                            h_flex()
                                .id(("home-conn", i))
                                .flex_none()
                                .h(px(ROW_H + 4.))
                                .gap_2()
                                .px_2()
                                .rounded(px(6.))
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
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.engine.delete_connection(&id).ok();
                                            this.connections = this.engine.connections();
                                            cx.notify();
                                        })),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.connect(c.clone(), window, cx)
                                }))
                        })),
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
