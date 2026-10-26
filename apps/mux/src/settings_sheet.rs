//! Settings, as a slab.
//!
//! What mux lets you set from the window is which agents ⌃a offers, so the
//! sheet lists them the way the launcher does: the name, then what it runs
//! or what it still needs, then on or off. A change saves as it is made.
//! The file behind it sits at the foot, with how to add an agent of your own.
//! The sheet reads the file each time it opens, so an agent added by hand
//! shows up without a restart, and a toggle never writes over an edit.

use super::*;

const SHEET_WIDTH: f32 = 600.0;
const ROW_HEIGHT: f32 = 30.0;
/// The name column, shared with the file line so the two read as a table.
const NAME_WIDTH: f32 = 120.0;

pub(super) struct SettingsSheet {
    focus: FocusHandle,
    selected: usize,
    /// What went wrong reading, saving or showing the file, when something did.
    problem: Option<String>,
    /// settings.json doesn't parse. Toggles would write over whatever the
    /// person is in the middle of, so they wait until it reads again.
    unreadable: bool,
}

impl MuxApp {
    /// Open settings, or close them if they are open: ⌘, does both.
    pub(super) fn toggle_settings_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_sheet.is_some() {
            self.close_settings_sheet(window, cx);
            return;
        }
        self.finish_tab_rename(true, window, cx);
        self.session_sheet = None;
        self.palette = None;
        let focus = cx.focus_handle();
        let deferred = focus.clone();
        window.on_next_frame(move |window, cx| deferred.focus(window, cx));
        self.settings_sheet = Some(SettingsSheet {
            focus,
            selected: 0,
            problem: None,
            unreadable: false,
        });
        self.reload_settings(window, cx);
        self.recheck_missing_agents(window, cx);
        self.mode = InputMode::Normal;
        cx.notify();
    }

    /// Pick up edits made to settings.json since it was last read: agents
    /// added under `agent_servers`, or ones turned off by hand.
    fn reload_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state_dir) = self.state_dir.as_ref() else {
            return;
        };
        let loaded = AppSettings::load(state_dir);
        if let Some(sheet) = self.settings_sheet.as_mut() {
            sheet.unreadable = loaded.is_err();
            sheet.problem = loaded.as_ref().err().map(|error| {
                format!(
                    "settings.json doesn't read ({}); fix it, then open this again",
                    error.root_cause()
                )
            });
        }
        if let Ok(settings) = loaded
            && settings != self.settings
        {
            self.profiles = merge_agent_profiles(&settings);
            self.settings = settings;
            Self::check_agent_availability(&self.profiles, window, cx);
        }
    }

    pub(super) fn close_settings_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_sheet.take().is_some() {
            self.restore_keyboard(window, cx);
            cx.notify();
        }
    }

    fn settings_sheet_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.close_settings_sheet(window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.modified() {
            return;
        }
        let count = self.profiles.len();
        let Some(sheet) = self.settings_sheet.as_mut() else {
            return;
        };
        match key {
            "up" | "k" => sheet.selected = wrapping_step(sheet.selected, -1, count),
            "down" | "j" => sheet.selected = wrapping_step(sheet.selected, 1, count),
            "space" | "enter" => {
                let row = sheet.selected;
                self.toggle_agent_setting(row);
            }
            "o" => self.show_settings_file(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn toggle_agent_setting(&mut self, row: usize) {
        let Some(profile_id) = self.profiles.get(row).map(|profile| profile.id.clone()) else {
            return;
        };
        if self
            .settings_sheet
            .as_ref()
            .is_some_and(|sheet| sheet.unreadable)
        {
            return;
        }
        let enabled = !self.settings.agent_enabled(&profile_id);
        self.settings.set_agent_enabled(&profile_id, enabled);
        let saved = self
            .state_dir
            .as_ref()
            .map_or(Ok(()), |state_dir| self.settings.save(state_dir));
        if let Some(sheet) = self.settings_sheet.as_mut() {
            sheet.problem = saved.err().map(|error| {
                error!(%error, "save settings");
                format!("couldn't save: {error:#}")
            });
        }
    }

    /// Show settings.json in Finder, writing it out first when there is no
    /// file yet to show.
    fn show_settings_file(&mut self) {
        let shown = self
            .state_dir
            .as_ref()
            .ok_or_else(|| anyhow!("there's no settings folder"))
            .and_then(|state_dir| {
                let path = state_dir.join("settings.json");
                if !path.exists() {
                    self.settings.save(state_dir)?;
                }
                Command::new("/usr/bin/open").arg("-R").arg(path).spawn()?;
                Ok(())
            });
        if let Err(error) = shown
            && let Some(sheet) = self.settings_sheet.as_mut()
        {
            sheet.problem = Some(format!("couldn't show it: {error:#}"));
        }
    }

    /// The sheet, centred under the strip, over a clear backdrop that closes
    /// it when clicked.
    pub(super) fn render_settings_sheet(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let sheet = self.settings_sheet.as_ref()?;
        let mut rows = v_flex().gap(px(2.0));
        for (index, profile) in self.profiles.iter().enumerate() {
            rows =
                rows.child(self.settings_agent_line(index, profile, index == sheet.selected, cx));
        }
        let panel = v_flex()
            .id("settings-sheet")
            .track_focus(&sheet.focus)
            .on_key_down(cx.listener(Self::settings_sheet_key))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .relative()
            .w(px(SHEET_WIDTH))
            .max_w(gpui::relative(0.92))
            .p(px(6.0))
            .pb(px(10.0))
            .gap(px(8.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(hairline(0.10))
            .bg(color(SURFACE))
            .shadow_lg()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
            .child(
                h_flex()
                    .h(px(28.0))
                    .pl(px(24.0))
                    .pr(px(10.0))
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child("settings"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(color(if sheet.unreadable { SIGNAL } else { FAINT_TEXT }))
                            .child(if sheet.unreadable {
                                "not saving"
                            } else {
                                "saved as you go"
                            }),
                    ),
            )
            .child(
                v_flex()
                    .child(
                        div()
                            .h(px(22.0))
                            .pl(px(24.0))
                            .text_size(px(12.0))
                            .text_color(color(FAINT_TEXT))
                            .child("agents ⌃a offers"),
                    )
                    .child(rows),
            )
            .child(self.settings_file_line(sheet))
            .child(settings_sheet_hints());
        let panel = if self.motion == MotionPreference::Reduced {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    "settings-sheet-in",
                    interface_animation(140),
                    |panel, delta| panel.opacity(delta).top(px(-6.0 * (1.0 - delta))),
                )
                .into_any_element()
        };
        Some(
            div()
                .id("settings-sheet-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(layout::TAB_BAR_HEIGHT + 48.0))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_settings_sheet(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn settings_agent_line(
        &self,
        index: usize,
        profile: &AgentProfile,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let enabled = self.settings.agent_enabled(&profile.id);
        let missing = self.agent_availability.get(&profile.id) == Some(&false);
        // What it runs, or, when it can't run here yet, what it needs.
        let detail = if missing {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.0))
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .text_color(color(SIGNAL))
                        .child(agent_view::install_hint(&profile.id)),
                )
                .when_some(
                    agent_view::install_command(&profile.id),
                    |detail, command| detail.child(agent_view::inline_command(command)),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(color(FAINT_TEXT))
                .child(profile.description.clone())
                .into_any_element()
        };
        h_flex()
            .id(SharedString::from(format!("settings-agent-{}", profile.id)))
            .relative()
            .h(px(ROW_HEIGHT))
            .pl(px(24.0))
            .pr(px(10.0))
            .gap(px(16.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(chosen, |line| line.bg(wash(0.08)))
            .when(!chosen, |line| line.hover(|line| line.bg(wash(0.04))))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(sheet) = this.settings_sheet.as_mut() {
                    sheet.selected = index;
                }
                this.toggle_agent_setting(index);
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .text_color(color(SIGNAL))
                    .when(!chosen, gpui::Styled::invisible)
                    .child("›"),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(NAME_WIDTH))
                    .truncate()
                    .when(chosen, |name| name.font_weight(FontWeight::BOLD))
                    .text_color(color(if enabled { TEXT } else { MUTED_TEXT }))
                    .child(agent_view::profile_title(profile)),
            )
            .child(detail)
            .child(
                h_flex()
                    .flex_none()
                    .w(px(28.0))
                    .justify_end()
                    .text_color(color(if enabled { SAGE } else { FAINT_TEXT }))
                    .child(if enabled { "on" } else { "off" }),
            )
            .into_any_element()
    }

    /// Where the settings live and how to add an agent there, or what went
    /// wrong with the file.
    fn settings_file_line(&self, sheet: &SettingsSheet) -> impl IntoElement {
        let path = self.state_dir.as_ref().map_or_else(
            || "nowhere yet".to_owned(),
            |state_dir| agent_view::home_relative(&state_dir.join("settings.json")),
        );
        let note = sheet.problem.as_ref().map_or_else(
            || {
                div()
                    .text_color(color(FAINT_TEXT))
                    .child("add your own under agent_servers, as in zed, then open this again")
            },
            |problem| div().text_color(color(SIGNAL)).child(problem.clone()),
        );
        h_flex()
            .items_start()
            .pl(px(24.0))
            .pr(px(10.0))
            .pt(px(4.0))
            .gap(px(16.0))
            .text_size(px(12.0))
            .child(
                div()
                    .flex_none()
                    .w(px(NAME_WIDTH))
                    .text_color(color(FAINT_TEXT))
                    .child("file"),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(div().truncate().text_color(color(MUTED_TEXT)).child(path))
                    .child(note.whitespace_normal()),
            )
    }
}

/// The sheet's keys, in gofer's `[key] label` form.
fn settings_sheet_hints() -> impl IntoElement {
    h_flex()
        .gap(px(12.0))
        .pl(px(24.0))
        .text_size(px(12.0))
        .text_color(color(FAINT_TEXT))
        .children(
            [
                ("↑↓", "choose"),
                ("space", "on/off"),
                ("o", "show in finder"),
                ("esc", "close"),
            ]
            .into_iter()
            .map(|(key, label)| {
                h_flex()
                    .flex_none()
                    .gap(px(6.0))
                    .child(kbd(key, color(MUTED_TEXT)))
                    .child(label)
            }),
        )
}
