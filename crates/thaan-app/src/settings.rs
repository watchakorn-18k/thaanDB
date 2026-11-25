//! The Settings dialog (⌘, or the gear in the title bar). Every choice applies at once
//! and is saved to the store in the background through `Engine::set_settings`; there is
//! no Save button. The values live in `ThaanApp::settings`, read by the rest of the app
//! (rows per page for new tables, the theme toggle). A child module of `app`.
//!
//! Two pages picked on the left: *General* (appearance, data) and *MCP* (how to plug AI
//! agents into the `thaan mcp` server, with a ready config for each client).

use super::*;
use gpui_kit::component::button::ButtonGroup;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::tab::{Tab, TabBar};
use std::path::PathBuf;
use thaan_core::store::{Settings, ThemePreference};

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum SettingsPage {
    #[default]
    General,
    Mcp,
}

/// View state of the Settings dialog that is not a setting itself.
#[derive(Default)]
pub(super) struct SettingsUi {
    page: SettingsPage,
    /// Index into `MCP_CLIENTS`.
    client: usize,
    /// Where the MCP server is; `None` until the background lookup returns.
    mcp: Option<McpServerInfo>,
    looking: bool,
}

#[derive(Clone)]
struct McpServerInfo {
    /// The `thaan` binary, if one was found.
    binary: Option<PathBuf>,
    /// `THAAN_HOME` as an absolute path, when the app runs with one (dev builds); the
    /// config passes it on so agents see the same connections as this window.
    home: Option<PathBuf>,
}

/// MCP clients and how each one takes a server: the place it goes, then the text.
const MCP_CLIENTS: [&str; 6] = [
    "Claude Code",
    "Claude Desktop",
    "Cursor",
    "VS Code",
    "Codex",
    "Windsurf",
];

/// Looks for the `thaan` binary: a release build next to this one (dev), the same folder
/// (the app bundle ships it beside `thaan-app`), then `PATH`. Disk work: background only.
fn find_thaan() -> Option<PathBuf> {
    let name = if cfg!(windows) { "thaan.exe" } else { "thaan" };
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates = vec![exe_dir.join("../release").join(name), exe_dir.join(name)];
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(name)));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.canonicalize().unwrap_or(p))
}

/// `(where it goes, text to paste)` for one client.
fn mcp_config(client: usize, info: &McpServerInfo) -> (String, String) {
    let cmd = info
        .binary
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "thaan".into());
    let home = info.home.as_ref().map(|p| p.display().to_string());
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    // JSON clients: `mcpServers` (VS Code: `servers` with a type).
    let json = |key: &str, typed: bool| {
        let mut server = serde_json::json!({ "command": cmd, "args": ["mcp"] });
        if typed {
            server["type"] = "stdio".into();
        }
        if let Some(h) = &home {
            server["env"] = serde_json::json!({ "THAAN_HOME": h });
        }
        let doc = serde_json::json!({ key: { "thaandb": server } });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    };
    match client {
        0 => {
            let shell = |s: &str| {
                if s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
                {
                    s.to_string()
                } else {
                    format!("'{}'", s.replace('\'', "'\\''"))
                }
            };
            let env = home
                .as_deref()
                .map(|h| format!(" -e THAAN_HOME={}", shell(h)))
                .unwrap_or_default();
            (
                "Run in a terminal".into(),
                format!("claude mcp add thaandb -s user{env} -- {} mcp", shell(&cmd)),
            )
        }
        1 => (
            if cfg!(target_os = "macos") {
                "Add to ~/Library/Application Support/Claude/claude_desktop_config.json".into()
            } else {
                "Add to claude_desktop_config.json (Settings, Developer, Edit config)".into()
            },
            json("mcpServers", false),
        ),
        2 => (
            "Add to ~/.cursor/mcp.json".into(),
            json("mcpServers", false),
        ),
        3 => (
            "Add to .vscode/mcp.json in your project".into(),
            json("servers", true),
        ),
        4 => {
            let env = home
                .as_deref()
                .map(|h| format!("\nenv = {{ THAAN_HOME = {} }}", q(h)))
                .unwrap_or_default();
            (
                "Add to ~/.codex/config.toml".into(),
                format!(
                    "[mcp_servers.thaandb]\ncommand = {}\nargs = [\"mcp\"]{env}",
                    q(&cmd)
                ),
            )
        }
        _ => (
            "Add to ~/.codeium/windsurf/mcp_config.json".into(),
            json("mcpServers", false),
        ),
    }
}

const THEMES: [(ThemePreference, &str); 3] = [
    (ThemePreference::System, "System"),
    (ThemePreference::Light, "Light"),
    (ThemePreference::Dark, "Dark"),
];
const TEXT_SIZES: [(u16, &str); 4] = [
    (15, "Small"),
    (16, "Default"),
    (17, "Large"),
    (18, "Larger"),
];
/// Below this dialog width the page list turns into tabs above the page.
const NARROW_SETTINGS: Pixels = px(600.);
const PAGE_SIZES: [u64; 4] = [25, 50, 100, 200];
const HISTORY: [usize; 4] = [100, 500, 1000, 5000];

impl ThaanApp {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // One dialog at a time: a second click or ⌘, while one is open does nothing.
        if window.has_active_dialog(cx) {
            return;
        }
        // Look for the server again each time: it may have been built since.
        self.settings_ui.mcp = None;
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            // Rebuilt every frame from the view's settings (the kit redraws dialogs), so
            // it also follows the window as it is resized: never wider than the window
            // less 16 on each side.
            let width = (window.viewport_size().width - px(32.)).min(px(720.));
            let body = this.update(cx, |app, cx| app.settings_dialog(width, cx));
            dialog.w(width).title("Settings").child(body)
        });
    }

    fn show_settings_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_ui.page = page;
        if page == SettingsPage::Mcp && self.settings_ui.mcp.is_none() && !self.settings_ui.looking
        {
            self.settings_ui.looking = true;
            let home = std::env::var_os("THAAN_HOME").map(PathBuf::from);
            cx.spawn(async move |this, cx| {
                let info = cx
                    .background_spawn(async move {
                        McpServerInfo {
                            binary: find_thaan(),
                            home: home.map(|h| std::path::absolute(&h).unwrap_or(h)),
                        }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.settings_ui.looking = false;
                    this.settings_ui.mcp = Some(info);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Page list on the left, the page on the right. In a narrow dialog the pages become
    /// a row of tabs above the page.
    fn settings_dialog(&mut self, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let page = self.settings_ui.page;
        if width < NARROW_SETTINGS {
            let body = match page {
                SettingsPage::General => self.settings_body(cx),
                SettingsPage::Mcp => self.mcp_body(cx),
            };
            return v_flex()
                .gap_4()
                .child(
                    TabBar::new("set-pages")
                        .segmented()
                        .small()
                        .selected_index(usize::from(page == SettingsPage::Mcp))
                        .child(Tab::new().label("General"))
                        .child(Tab::new().label("MCP"))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            let p = if *ix == 1 {
                                SettingsPage::Mcp
                            } else {
                                SettingsPage::General
                            };
                            this.show_settings_page(p, cx)
                        })),
                )
                .child(v_flex().min_w_0().min_h(px(420.)).child(body))
                .into_any_element();
        }
        let nav = |id: &'static str,
                   label: &'static str,
                   ic: &str,
                   p: SettingsPage,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .icon(icon(ic))
                .label(label)
                .selected(page == p)
                .on_click(cx.listener(move |this, _, _, cx| this.show_settings_page(p, cx)))
        };
        let body = match page {
            SettingsPage::General => self.settings_body(cx),
            SettingsPage::Mcp => self.mcp_body(cx),
        };
        h_flex()
            .items_start()
            .gap_6()
            .child(
                v_flex()
                    .w(px(148.))
                    .flex_none()
                    .gap_0p5()
                    .child(nav(
                        "set-page-general",
                        "General",
                        "sliders-horizontal",
                        SettingsPage::General,
                        cx,
                    ))
                    .child(nav("set-page-mcp", "MCP", "bot", SettingsPage::Mcp, cx)),
            )
            // Same height on both pages, so switching does not make the dialog jump.
            .child(v_flex().flex_1().min_w_0().min_h(px(420.)).child(body))
            .into_any_element()
    }

    fn mcp_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let hint = |text: String| div().text_xs().text_color(muted).child(text);
        // A mono text block with a copy button in its top-right corner.
        let well = |id: &'static str, text: String| {
            h_flex()
                .items_start()
                .gap_2()
                .p_3()
                .rounded(px(8.))
                .bg(theme.muted)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .font_family(mono.clone())
                        .whitespace_normal()
                        .child(text.clone()),
                )
                .child(Clipboard::new(id).value(text).tooltip("Copy"))
        };
        let section = |title: &str| div().text_sm().font_medium().child(title.to_string());

        let info = self.settings_ui.mcp.clone();
        let (status, status_color) = match &info {
            None => ("Looking for thaan", muted),
            Some(i) if i.binary.is_some() => ("Ready", theme.success),
            Some(_) => ("Not built yet", theme.warning),
        };
        let server = match info.as_ref().map(|i| i.binary.clone()) {
            Some(Some(p)) => well("mcp-path", p.display().to_string()).into_any_element(),
            Some(None) => v_flex()
                .gap_2()
                .child(hint(
                    "Build the thaan command line tool, then open this page again".into(),
                ))
                .child(well(
                    "mcp-build",
                    "cargo build --release -p thaan-cli".into(),
                ))
                .into_any_element(),
            None => div().h(px(40.)).into_any_element(),
        };
        let client = self.settings_ui.client;
        let config = info.as_ref().map(|i| mcp_config(client, i));
        let store_path = info
            .as_ref()
            .and_then(|i| i.home.clone().map(|h| h.join("store.json")))
            .or_else(thaan_core::store::Store::default_path)
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        v_flex()
            .gap_3()
            .child(
                v_flex().gap_1().child(section("MCP server")).child(hint(
                    "AI agents such as Claude Code or Cursor use your saved connections \
                         through thaan mcp. They can do anything you can here; mark a \
                         connection Read-only to keep them from writing to it"
                        .into(),
                )),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().child("Server"))
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(div().size(px(6.)).rounded_full().bg(status_color))
                                    .child(status),
                            ),
                    )
                    .child(server),
            )
            .child(div().h(px(1.)).my_1().bg(theme.border))
            .child(section("Add to a client"))
            .child(
                TabBar::new("mcp-clients")
                    .segmented()
                    .small()
                    // Too narrow for all six: the rest scroll, and a menu lists them.
                    .menu(true)
                    .selected_index(client)
                    .children(MCP_CLIENTS.iter().map(|c| Tab::new().label(*c)))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.settings_ui.client = *ix;
                        cx.notify();
                    })),
            )
            .when_some(config, |el, (place, text)| {
                el.child(hint(place)).child(well("mcp-config", text))
            })
            .child(
                div()
                    .pt_1()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .truncate()
                    .child(format!("Connections from {store_path}")),
            )
            .into_any_element()
    }

    /// Changes the settings: applies them now, saves them off the UI thread.
    pub(super) fn update_settings(
        &mut self,
        change: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.settings.clone();
        change(&mut next);
        let next = next.clamped();
        if next == self.settings {
            return;
        }
        let appearance =
            next.theme != self.settings.theme || next.font_size != self.settings.font_size;
        let history = next.history_limit != self.settings.history_limit;
        self.settings = next.clone();
        if appearance {
            theme::apply_settings(&next, Some(window), cx);
        }
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { engine.set_settings(next) })
                .await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(_) if history => this.refresh_home_history(),
                    Ok(_) => {}
                    Err(e) => this.set_status(format!("Settings not saved: {e}"), true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The theme button and palette command: flip between light and dark and keep it.
    pub(super) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if theme::is_dark(cx) {
            ThemePreference::Light
        } else {
            ThemePreference::Dark
        };
        self.update_settings(|s| s.theme = next, window, cx);
    }

    /// Re-applies a System theme when the OS switches between light and dark.
    pub(super) fn follow_system_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.theme == ThemePreference::System {
            theme::apply_settings(&self.settings, Some(window), cx);
        }
    }

    fn settings_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = self.settings.clone();
        let section = |title: &str| {
            div()
                .pt_2()
                .text_sm()
                .font_medium()
                .child(title.to_string())
        };
        // A label (and a muted hint under it) on the left, the choice on the right.
        let row = |label: &str, hint: &str, control: AnyElement| {
            h_flex()
                .min_h(px(44.))
                .flex_wrap()
                .gap_x_4()
                .gap_y_2()
                .justify_between()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(180.))
                        .py_1()
                        .gap_0p5()
                        .child(div().text_sm().child(label.to_string()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(hint.to_string()),
                        ),
                )
                .child(div().flex_none().child(control))
        };
        let segmented = |id: &'static str,
                         labels: Vec<String>,
                         selected: Option<usize>,
                         cx: &mut Context<Self>,
                         pick: fn(&mut Settings, usize)| {
            ButtonGroup::new(id)
                .outline()
                .small()
                .children(
                    labels
                        .into_iter()
                        .enumerate()
                        .map(|(i, l)| Button::new((id, i)).label(l).selected(selected == Some(i))),
                )
                .on_click(cx.listener(move |this, clicks: &Vec<usize>, window, cx| {
                    if let Some(&i) = clicks.first() {
                        this.update_settings(|s| pick(s, i), window, cx);
                    }
                }))
                .into_any_element()
        };
        let theme_ix = THEMES.iter().position(|(t, _)| *t == s.theme);
        let size_ix = TEXT_SIZES.iter().position(|(n, _)| *n == s.font_size);
        let page_ix = PAGE_SIZES.iter().position(|n| *n == s.page_size);
        let hist_ix = HISTORY.iter().position(|n| *n == s.history_limit);
        let store_path = thaan_core::store::Store::default_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        v_flex()
            .gap_1()
            .child(section("Appearance"))
            .child(row(
                "Theme",
                "System follows the operating system",
                segmented(
                    "set-theme",
                    THEMES.iter().map(|(_, l)| l.to_string()).collect(),
                    theme_ix,
                    cx,
                    |s, i| s.theme = THEMES[i].0,
                ),
            ))
            .child(row(
                "Text size",
                "Interface, grid and editor",
                segmented(
                    "set-text",
                    TEXT_SIZES.iter().map(|(_, l)| l.to_string()).collect(),
                    size_ix,
                    cx,
                    |s, i| s.font_size = TEXT_SIZES[i].0,
                ),
            ))
            .child(div().h(px(1.)).my_2().bg(theme.border))
            .child(section("Data"))
            .child(row(
                "Rows per page",
                "For tables you open next",
                segmented(
                    "set-page",
                    PAGE_SIZES.iter().map(|n| n.to_string()).collect(),
                    page_ix,
                    cx,
                    |s, i| s.page_size = PAGE_SIZES[i],
                ),
            ))
            .child(row(
                "Query history",
                "Older queries are dropped past this",
                segmented(
                    "set-history",
                    HISTORY.iter().map(|n| thousands(*n as u64)).collect(),
                    hist_ix,
                    cx,
                    |s, i| s.history_limit = HISTORY[i],
                ),
            ))
            .child(
                div()
                    .pt_3()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(format!("Saved in {store_path}")),
            )
            .into_any_element()
    }
}

/// `1000` → `1,000`.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
