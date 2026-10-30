use super::*;
use iced::widget::column;

pub(super) fn system_dark() -> Option<bool> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut value = 1u32;
    let mut size = 4u32;
    // SAFETY: null-terminated UTF-16 names and a writable DWORD of the supplied size.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(value == 0)
}

impl App {
    pub(super) fn dark(&self) -> bool {
        match self.app_settings.theme {
            ThemePreference::Light => false,
            ThemePreference::Dark => true,
            ThemePreference::System => self.system_dark,
        }
    }
    pub fn theme(&self, _: window::Id) -> iced::Theme {
        fluent::theme(self.dark())
    }
    pub(super) fn colors(&self) -> Palette {
        Palette::new(self.dark())
    }
    pub(super) fn sync_theme(&mut self) -> Task<Message> {
        let dark = self.dark();
        if self.native_dark == Some(dark) {
            return Task::none();
        }
        self.native_dark = Some(dark);
        Task::batch(std::iter::once(self.main).chain(self.pending).map(|id| {
            window::run(id, move |window| backdrop::configure(window, dark))
                .map(move |result| Message::BackdropReady(id, dark, result))
        }))
    }
    pub(super) fn save_app_settings(&mut self, settings: AppSettings) {
        if self.settings_enabled() && settings != self.app_settings {
            self.settings_saving = Some(settings.clone());
            self.send(Command::AppSettings(settings));
        }
    }
    fn settings_enabled(&self) -> bool {
        self.service.is_some()
            && self.app_settings_loaded
            && self.settings_saving.is_none()
            && !self.updates.installing()
    }
    pub(super) fn settings_key(&mut self, key: iced::keyboard::key::Named) {
        use iced::keyboard::key::Named;
        if key == Named::Tab {
            self.setting_focus = 1 - self.setting_focus;
            return;
        }
        if !self.settings_enabled() {
            return;
        }
        let mut settings = self.app_settings.clone();
        let adjust = |value: usize, min: usize, max: usize| match key {
            Named::ArrowUp => value.saturating_sub(1).max(min),
            Named::ArrowDown => (value + 1).min(max),
            Named::Home => min,
            Named::End => max,
            _ => value,
        };
        if self.setting_focus == 0 {
            settings.concurrency = adjust(settings.concurrency, 1, AppSettings::max_concurrency());
        } else {
            let themes = [
                ThemePreference::Light,
                ThemePreference::Dark,
                ThemePreference::System,
            ];
            let current = themes.iter().position(|t| *t == settings.theme).unwrap();
            settings.theme = themes[adjust(current, 0, 2)];
        }
        self.save_app_settings(settings);
    }
    pub(super) fn settings_view(&self) -> Element<'_, Message> {
        let colors = self.colors();
        let selected = self.settings_saving.as_ref().unwrap_or(&self.app_settings);
        let enabled = self.settings_enabled();
        let concurrency = pick_list(
            (1..=AppSettings::max_concurrency()).collect::<Vec<usize>>(),
            Some(selected.concurrency),
            Message::Concurrency,
        )
        .on_open(Message::SettingFocus(0))
        .style(move |theme, status| setting_style(theme, status, self.setting_focus == 0))
        .menu_style(menu)
        .width(180)
        .padding(10);
        let theme = pick_list(
            [
                ThemePreference::Light,
                ThemePreference::Dark,
                ThemePreference::System,
            ],
            Some(selected.theme),
            Message::Theme,
        )
        .on_open(Message::SettingFocus(1))
        .style(move |theme, status| setting_style(theme, status, self.setting_focus == 1))
        .menu_style(menu)
        .width(180)
        .padding(10);
        let concurrency: Element<_> = if enabled {
            concurrency.into()
        } else {
            container(text(selected.concurrency.to_string()).color(colors.muted))
                .padding(10)
                .width(180)
                .style(inset)
                .into()
        };
        let theme: Element<_> = if enabled {
            theme.into()
        } else {
            container(text(selected.theme.to_string()).color(colors.muted))
                .padding(10)
                .width(180)
                .style(inset)
                .into()
        };
        scrollable(
            column![
                container(
                    column![
                        heading("视频处理", 17),
                        row![
                            column![
                                text("并发线程数").size(14),
                                text("限制同时进行的内容校验与封面生成任务数")
                                    .size(12)
                                    .color(colors.muted)
                            ]
                            .spacing(8)
                            .width(Fill),
                            concurrency
                        ]
                        .spacing(16)
                        .align_y(Alignment::Center),
                        text("修改后对后续任务生效，正在处理的视频会继续完成。")
                            .size(12)
                            .color(colors.muted),
                    ]
                    .spacing(20)
                )
                .padding(24)
                .width(Fill)
                .style(panel),
                container(
                    column![
                        heading("外观", 17),
                        row![
                            column![
                                text("应用主题").size(14),
                                text("选择浅色、深色，或跟随 Windows 应用主题")
                                    .size(12)
                                    .color(colors.muted)
                            ]
                            .spacing(8)
                            .width(Fill),
                            theme
                        ]
                        .spacing(16)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(20)
                )
                .padding(24)
                .width(Fill)
                .style(panel),
                text(if self.settings_saving.is_some() {
                    "正在保存设置…"
                } else if !self.app_settings_loaded {
                    "正在读取设置…"
                } else {
                    "设置自动保存"
                })
                .size(12)
                .color(colors.muted),
                text("Tab 切换选项，↑ / ↓ 调整数值")
                    .size(12)
                    .color(colors.muted),
            ]
            .spacing(20)
            .width(Fill),
        )
        .height(Fill)
        .into()
    }
}

fn setting_style(
    theme: &iced::Theme,
    status: iced::widget::pick_list::Status,
    focused: bool,
) -> iced::widget::pick_list::Style {
    let mut style = select(theme, status);
    if focused {
        style.background = palette(theme).tint.into();
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrency_keyboard_uses_cpu_limit() {
        use iced::keyboard::key::Named;
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().to_path_buf(), None, None);
        app.app_settings_loaded = true;
        app.setting_focus = 0;
        let max = AppSettings::max_concurrency();
        for (value, key, expected) in [
            (1, Named::End, max),
            (max, Named::ArrowDown, max),
            (max, Named::Home, 1),
            (1, Named::ArrowUp, 1),
            (1, Named::ArrowDown, 2.min(max)),
            (max, Named::ArrowUp, max.saturating_sub(1).max(1)),
        ] {
            app.app_settings.concurrency = value;
            app.settings_saving = None;
            app.settings_key(key);
            let selected = app.settings_saving.as_ref().unwrap_or(&app.app_settings);
            assert_eq!(selected.concurrency, expected);
        }
    }

    #[test]
    fn settings_navigation_keeps_library_state_and_system_theme_is_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = App::boot(dir.path().to_path_buf(), None, None);
        app.main_pane.search = "保留搜索".into();
        app.main_pane.page = 3;
        app.main_pane.selected.insert(7);
        let _ = app.update(Message::ShowSettings);
        assert_eq!(app.page, MainPage::Settings);
        assert_eq!(app.main_pane.page, 3);
        assert!(app.main_pane.selected.contains(&7));
        app.settings_key(iced::keyboard::key::Named::Tab);
        assert_eq!(app.setting_focus, 1);
        let _ = app.update(Message::FocusSearch(app.main));
        assert_eq!(app.page, MainPage::Library);
        assert_eq!(app.main_pane.search, "保留搜索");
        app.app_settings.theme = ThemePreference::System;
        for dark in [true, false] {
            app.system_dark = dark;
            assert_eq!(app.dark(), dark);
            assert_eq!(app.theme(app.main).extended_palette().is_dark, dark);
        }
        app.app_settings.theme = ThemePreference::Dark;
        assert!(app.dark());
        app.app_settings.theme = ThemePreference::Light;
        app.system_dark = true;
        assert!(!app.dark());
    }
}
