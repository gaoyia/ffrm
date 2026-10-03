use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, ease_in_out, fill, point, prelude::*, px, rgb, size, Animation, AnimationExt, App,
    Application, Bounds, Context, ExternalPaths, FontWeight, IntoElement, ScrollHandle,
    TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowOptions,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use ffrm::Inspection;

use crate::config::{self, Config, Lang};
use crate::menu::{self, MenuState};
use crate::update::{self, Status};

const CANVAS: u32 = 0x12110f;
const INK: u32 = 0x1c1a17;
const LINE: u32 = 0x3c362e;
const CREAM: u32 = 0xf4efe6;
const STONE: u32 = 0xa89b8c;
const BRASS: u32 = 0xc6a57a;
const DELETE: u32 = 0xb5523e;
const DELETE_PRESSED: u32 = 0x8c3f2e;
const LIST_RADIUS: f32 = 24.;

fn text(lang: Lang, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

fn window_title(lang: Lang) -> &'static str {
    text(lang, "文件强制解锁删除工具", "Force Unlock and Delete")
}

fn menu_label(lang: Lang) -> &'static str {
    text(lang, "强制解锁删除", "Force Unlock and Delete")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Queue,
    Settings,
}

enum UpdateState {
    Checking,
    Current,
    Available(String),
    Installing,
    Missing,
    Failed,
    UpdateFailed,
}

impl From<Status> for UpdateState {
    fn from(status: Status) -> Self {
        match status {
            Status::Current => UpdateState::Current,
            Status::Available(version) => UpdateState::Available(version),
            Status::Missing => UpdateState::Missing,
            Status::Failed => UpdateState::Failed,
        }
    }
}

struct Note {
    ok: bool,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Refresh,
    Unlock,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Checking,
    Free,
    Locked,
    Critical,
    Unlisted,
    Error,
}

#[derive(Clone)]
struct Failure {
    name: String,
    reason: String,
    unlock: bool,
}

struct Item {
    path: PathBuf,
    mark: Mark,
}

pub struct Desktop {
    items: Vec<Item>,
    busy: bool,
    job: Option<Job>,
    pending_refresh: bool,
    unlock: bool,
    force: bool,
    pinned: bool,
    lang: Lang,
    lang_anim: usize,
    failures: Vec<Failure>,
    scroll: ScrollHandle,
    scroll_grab: Option<gpui::Pixels>,
    page: Page,
    config_error: Option<String>,
    menu_state: MenuState,
    menu_note: Option<Note>,
    win11: bool,
    update_epoch: u64,
    update: UpdateState,
}

impl Desktop {
    fn from_config(config: Config, config_error: Option<String>) -> Self {
        Self {
            items: Vec::new(),
            busy: false,
            job: None,
            pending_refresh: false,
            unlock: config.unlock,
            force: config.force,
            pinned: config.pinned,
            lang: config.lang,
            lang_anim: 0,
            failures: Vec::new(),
            scroll: ScrollHandle::new(),
            scroll_grab: None,
            page: Page::Queue,
            config_error,
            menu_state: MenuState::Absent,
            menu_note: None,
            win11: menu::windows_11(),
            update_epoch: 0,
            update: UpdateState::Checking,
        }
    }

    fn choose_language(&mut self, lang: Lang, window: &mut Window, cx: &mut Context<Self>) {
        if self.lang == lang {
            return;
        }
        self.lang = lang;
        self.lang_anim = self.lang_anim.wrapping_add(1);
        window.set_window_title(window_title(lang));
        self.persist();
        if menu::state() == MenuState::Current {
            if let Err(error) = menu::install(menu_label(lang), lang) {
                self.menu_note = Some(Note {
                    ok: false,
                    text: error,
                });
                self.menu_state = menu::state();
            }
        }
        if self.items.is_empty() {
            cx.notify();
            return;
        }
        if self.busy {
            self.pending_refresh = true;
            cx.notify();
            return;
        }
        self.run_job(cx, Job::Refresh);
    }

    fn waiting_for_unlock(&self) -> bool {
        match self.job {
            Some(Job::Unlock) => true,
            Some(Job::Delete) => self.unlock,
            _ => false,
        }
    }

    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pinned = !self.pinned;
        set_topmost(window, self.pinned);
        self.persist();
        cx.notify();
    }

    fn toggle_unlock(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.unlock = !self.unlock;
        self.persist();
        cx.notify();
    }

    fn toggle_force(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.force = !self.force;
        self.persist();
        cx.notify();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Settings {
            self.page = Page::Queue;
        } else {
            self.page = Page::Settings;
            self.menu_state = menu::state();
            self.menu_note = None;
            self.check_update(cx);
        }
        cx.notify();
    }

    fn check_update(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update, UpdateState::Installing) {
            return;
        }
        self.update_epoch = self.update_epoch.wrapping_add(1);
        let epoch = self.update_epoch;
        self.update = UpdateState::Checking;
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async { update::check(env!("CARGO_PKG_VERSION")) })
                .await;
            this.update(cx, |this, cx| {
                if this.update_epoch == epoch && !matches!(this.update, UpdateState::Installing) {
                    this.update = UpdateState::from(status);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.update, UpdateState::Available(_)) {
            return;
        }
        self.update = UpdateState::Installing;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async { update::install() }).await;
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    cx.quit();
                } else if matches!(this.update, UpdateState::Installing) {
                    this.update = UpdateState::UpdateFailed;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn install_menu(&mut self, cx: &mut Context<Self>) {
        let lang = self.lang;
        self.menu_note = Some(match menu::install(menu_label(lang), lang) {
            Ok(()) => Note {
                ok: true,
                text: text(lang, "已添加到右键菜单", "Added to the context menu").to_string(),
            },
            Err(error) => Note {
                ok: false,
                text: error,
            },
        });
        self.menu_state = menu::state();
        cx.notify();
    }

    fn remove_menu(&mut self, cx: &mut Context<Self>) {
        let lang = self.lang;
        self.menu_note = Some(match menu::remove(lang) {
            Ok(()) => Note {
                ok: true,
                text: text(lang, "已从右键菜单移除", "Removed from the context menu").to_string(),
            },
            Err(error) => Note {
                ok: false,
                text: error,
            },
        });
        self.menu_state = menu::state();
        cx.notify();
    }

    fn persist(&mut self) {
        let config = Config {
            lang: self.lang,
            unlock: self.unlock,
            force: self.force,
            pinned: self.pinned,
        };
        self.config_error = config.save().err();
    }

    fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let mut added = false;
        for path in paths {
            if self.items.iter().any(|item| item.path == *path) {
                continue;
            }
            self.items.push(Item {
                path: path.clone(),
                mark: Mark::Checking,
            });
            added = true;
        }
        if !added {
            return;
        }
        if self.busy {
            self.pending_refresh = true;
            cx.notify();
            return;
        }
        self.run_job(cx, Job::Refresh);
    }

    fn run_job(&mut self, cx: &mut Context<Self>, job: Job) {
        if self.busy || (self.items.is_empty() && !matches!(job, Job::Refresh)) {
            return;
        }
        self.busy = true;
        self.job = Some(job);
        self.failures.clear();
        let paths: Vec<PathBuf> = self.items.iter().map(|item| item.path.clone()).collect();
        let force = self.force;
        let unlock = self.unlock;
        let hold_mask = self.waiting_for_unlock();
        let started = Instant::now();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { perform(job, paths, unlock, force) })
                .await;
            if hold_mask {
                let remain = Duration::from_millis(600).saturating_sub(started.elapsed());
                if !remain.is_zero() {
                    gpui::Timer::after(remain).await;
                }
            }
            this.update(cx, |this, cx| {
                this.apply(outcome);
                this.busy = false;
                this.job = None;
                let refresh = this.pending_refresh;
                this.pending_refresh = false;
                if refresh && !this.items.is_empty() {
                    this.run_job(cx, Job::Refresh);
                } else {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Refreshed(details) => {
                for (path, mark) in details {
                    if let Some(item) = self.items.iter_mut().find(|item| item.path == path) {
                        item.mark = mark;
                    }
                }
            }
            Outcome::Done {
                failures,
                removed,
                details,
            } => {
                self.items
                    .retain(|item| !removed.iter().any(|path| path == &item.path));
                for (path, mark) in details {
                    if let Some(item) = self.items.iter_mut().find(|item| item.path == path) {
                        item.mark = mark;
                    }
                }
                self.failures = failures;
            }
        }
    }
}

enum Outcome {
    Refreshed(Vec<(PathBuf, Mark)>),
    Done {
        failures: Vec<Failure>,
        removed: Vec<PathBuf>,
        details: Vec<(PathBuf, Mark)>,
    },
}

fn perform(job: Job, paths: Vec<PathBuf>, unlock: bool, force: bool) -> Outcome {
    match job {
        Job::Refresh => {
            let details = paths
                .into_iter()
                .map(|path| {
                    let mark = classify_path(&path);
                    (path, mark)
                })
                .collect();
            Outcome::Refreshed(details)
        }
        Job::Unlock => {
            let mut failures = Vec::new();
            for path in &paths {
                if let Err(error) = ffrm::release(path, force) {
                    failures.push(failure(path, error, true));
                }
            }
            let details = paths
                .iter()
                .map(|path| (path.clone(), classify_path(path)))
                .collect();
            Outcome::Done {
                failures,
                removed: Vec::new(),
                details,
            }
        }
        Job::Delete => {
            let mut failures = Vec::new();
            let mut removed = Vec::new();
            for path in &paths {
                if unlock {
                    if let Err(error) = ffrm::release(path, force) {
                        failures.push(failure(path, error, true));
                        continue;
                    }
                }
                match ffrm::delete_path(path, true) {
                    Ok(_) => removed.push(path.clone()),
                    Err(error) => failures.push(failure(path, error, false)),
                }
            }
            let details = paths
                .iter()
                .filter(|path| !removed.iter().any(|deleted| deleted == *path))
                .map(|path| (path.clone(), classify_path(path)))
                .collect();
            Outcome::Done {
                failures,
                removed,
                details,
            }
        }
    }
}

fn classify_path(path: &Path) -> Mark {
    match ffrm::inspect(path) {
        Ok(inspection) => classify(&inspection),
        Err(_) => Mark::Error,
    }
}

fn classify(inspection: &Inspection) -> Mark {
    if inspection.lockers.is_empty() && !inspection.sharing_violation {
        return Mark::Free;
    }
    if inspection.lockers.is_empty() {
        return Mark::Unlisted;
    }
    if inspection.lockers.iter().any(ffrm::is_critical) {
        return Mark::Critical;
    }
    Mark::Locked
}

fn failure(path: &Path, reason: String, unlock: bool) -> Failure {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Failure {
        name,
        reason,
        unlock,
    }
}

fn window_hwnd(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return None;
    };
    Some(win32.hwnd.get() as windows_sys::Win32::Foundation::HWND)
}

fn apply_window_icon(window: &Window) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, WM_SETICON,
    };
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    if module.is_null() {
        return;
    }
    unsafe {
        let big = LoadImageW(module, 1 as *const u16, IMAGE_ICON, 32, 32, 0);
        let small = LoadImageW(module, 1 as *const u16, IMAGE_ICON, 16, 16, 0);
        if !big.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        }
        if !small.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
    }
}

fn set_topmost(window: &Window, topmost: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let after = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    unsafe {
        SetWindowPos(
            hwnd,
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

fn toggle_zoom(window: &Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_MAXIMIZE, SW_RESTORE};
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };
    let command = if window.is_maximized() {
        SW_RESTORE
    } else {
        SW_MAXIMIZE
    };
    unsafe {
        ShowWindowAsync(hwnd, command);
    }
}

impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy;
        let has_items = !self.items.is_empty();
        let count = self.items.len();
        div()
            .id("ffrm-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(CREAM))
            .font_family("Microsoft YaHei UI")
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, _window, cx| {
                this.page = Page::Queue;
                this.add_paths(dropped.paths(), cx);
            }))
            .child(title_bar(self, window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(self.page == Page::Queue, |layer| {
                        layer
                            .child(queue(self, busy, has_items, count, cx))
                            .when(!self.failures.is_empty(), |layer| {
                                layer.child(failure_dialog(self, cx))
                            })
                    })
                    .when(self.page == Page::Settings, |layer| {
                        layer.child(settings_page(self, cx))
                    }),
            )
    }
}

fn title_bar(
    desktop: &Desktop,
    window: &mut Window,
    cx: &mut Context<Desktop>,
) -> impl IntoElement {
    let lang = desktop.lang;
    let pinned = desktop.pinned;
    let maximized = window.is_maximized();
    div()
        .h(px(48.))
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .id("window-drag")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .px_8()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .window_control_area(WindowControlArea::Drag)
                .child(div().truncate().child(window_title(lang))),
        )
        .child(language_switch(lang, desktop.lang_anim, cx))
        .child(settings_button(desktop.page == Page::Settings, lang, cx))
        .child(pin_button(pinned, text(lang, "前置", "Pin"), cx))
        .child(chrome_button(
            "window-min",
            ChromeIcon::Minimize,
            false,
            cx.listener(|_this, _event, window, _cx| {
                window.minimize_window();
            }),
        ))
        .child(chrome_button(
            "window-max",
            if maximized {
                ChromeIcon::Restore
            } else {
                ChromeIcon::Maximize
            },
            false,
            cx.listener(|this, _event, window, cx| {
                let pinned = this.pinned;
                toggle_zoom(window);
                window.on_next_frame(move |window, _cx| {
                    if pinned {
                        set_topmost(window, true);
                    }
                    window.refresh();
                });
                cx.notify();
            }),
        ))
        .child(chrome_button(
            "window-close",
            ChromeIcon::Close,
            true,
            cx.listener(|_this, _event, window, _cx| {
                window.remove_window();
            }),
        ))
}

fn language_switch(lang: Lang, generation: usize, cx: &mut Context<Desktop>) -> impl IntoElement {
    const SLOT: f32 = 48.;
    const PAD: f32 = 2.;
    let target = if lang == Lang::En { 1. } else { 0. };
    let origin = if generation == 0 { target } else { 1. - target };
    let duration = if generation == 0 { 1 } else { 220 };
    let animation = Animation::new(Duration::from_millis(duration)).with_easing(ease_in_out);
    div()
        .relative()
        .mr_3()
        .w(px(PAD * 2. + SLOT * 2.))
        .h(px(28.))
        .rounded_full()
        .bg(rgb(INK))
        .child(
            div()
                .absolute()
                .top(px(PAD))
                .w(px(SLOT))
                .h(px(24.))
                .rounded_full()
                .bg(rgb(CREAM))
                .with_animation(
                    ("lang-thumb", generation),
                    animation.clone(),
                    move |thumb, delta| {
                        let x = origin + (target - origin) * delta;
                        thumb.left(px(PAD + x * SLOT))
                    },
                ),
        )
        .child(
            div()
                .absolute()
                .top(px(PAD))
                .left(px(PAD))
                .h(px(24.))
                .flex()
                .child(language_option(
                    "lang-zh",
                    "中文",
                    false,
                    Lang::Zh,
                    origin,
                    target,
                    generation,
                    animation.clone(),
                    cx,
                ))
                .child(language_option(
                    "lang-en",
                    "EN",
                    true,
                    Lang::En,
                    origin,
                    target,
                    generation,
                    animation,
                    cx,
                )),
        )
}

fn language_option(
    id: &'static str,
    label: &'static str,
    english: bool,
    target: Lang,
    origin: f32,
    to: f32,
    generation: usize,
    animation: Animation,
    cx: &mut Context<Desktop>,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(48.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _event, window, cx| {
            this.choose_language(target, window, cx);
        }))
        .child(label)
        .with_animation((id, generation), animation, move |item, delta| {
            let x = origin + (to - origin) * delta;
            let coverage = if english { x } else { 1. - x };
            item.text_size(px(12.))
                .text_color(rgb(mix_rgb(STONE, CANVAS, coverage)))
        })
}

fn mix_rgb(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0., 1.);
    let channel = |shift: u32| {
        let start = ((from >> shift) & 0xff) as f32;
        let end = ((to >> shift) & 0xff) as f32;
        (start + (end - start) * t).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn settings_button(open: bool, lang: Lang, cx: &mut Context<Desktop>) -> impl IntoElement {
    let mut button = div()
        .id("window-settings")
        .w(px(88.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .cursor_pointer()
        .on_click(cx.listener(|this, _event, _window, cx| {
            this.toggle_settings(cx);
        }));
    if open {
        button = button.bg(rgb(BRASS)).text_color(rgb(CANVAS));
    } else {
        button = button.text_color(rgb(STONE));
    }
    button
        .hover(move |style| {
            if open {
                style.bg(rgb(0xd4b896)).text_color(rgb(CANVAS))
            } else {
                style.bg(rgb(INK)).text_color(rgb(CREAM))
            }
        })
        .active(|style| style.bg(rgb(0x8d7350)).text_color(rgb(CREAM)))
        .child(text(lang, "设置", "Settings"))
}

fn pin_button(pinned: bool, label: &'static str, cx: &mut Context<Desktop>) -> impl IntoElement {
    let mut button = div()
        .id("window-pin")
        .w(px(64.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .cursor_pointer()
        .on_click(cx.listener(|this, _event, window, cx| {
            this.toggle_pin(window, cx);
        }));
    if pinned {
        button = button.bg(rgb(BRASS)).text_color(rgb(CANVAS));
    } else {
        button = button.text_color(rgb(STONE));
    }
    button
        .hover(move |style| {
            if pinned {
                style.bg(rgb(0xd4b896)).text_color(rgb(CANVAS))
            } else {
                style.bg(rgb(INK)).text_color(rgb(CREAM))
            }
        })
        .active(|style| style.bg(rgb(0x8d7350)).text_color(rgb(CREAM)))
        .child(label)
}

#[derive(Clone, Copy)]
enum ChromeIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn chrome_glyph(icon: ChromeIcon) -> &'static str {
    match icon {
        ChromeIcon::Minimize => "\u{E921}",
        ChromeIcon::Maximize => "\u{E922}",
        ChromeIcon::Restore => "\u{E923}",
        ChromeIcon::Close => "\u{E8BB}",
    }
}

fn chrome_button(
    id: &'static str,
    icon: ChromeIcon,
    close: bool,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(STONE))
        .cursor_pointer()
        .hover(move |style| {
            if close {
                style.bg(rgb(DELETE)).text_color(rgb(CREAM))
            } else {
                style.bg(rgb(INK)).text_color(rgb(CREAM))
            }
        })
        .active(move |style| {
            if close {
                style.bg(rgb(DELETE_PRESSED)).text_color(rgb(CREAM))
            } else {
                style.bg(rgb(0x2a261f)).text_color(rgb(BRASS))
            }
        })
        .on_click(click)
        .child(
            div()
                .font_family("Segoe Fluent Icons")
                .text_size(px(10.))
                .line_height(px(10.))
                .child(chrome_glyph(icon)),
        )
}

fn queue_count(count: usize) -> impl IntoElement {
    let filled = count > 0;
    div()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(6.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .bg(rgb(if filled { 0x2a241c } else { INK }))
        .text_color(rgb(if filled { BRASS } else { STONE }))
        .child(count.to_string())
}

fn queue(
    desktop: &Desktop,
    busy: bool,
    has_items: bool,
    count: usize,
    cx: &mut Context<Desktop>,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .px_8()
        .pb_6()
        .gap_4()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().text_color(rgb(STONE)).child(text(
                    desktop.lang,
                    "队列",
                    "Queue",
                )))
                .child(queue_count(count))
                .when(busy, |row| {
                    row.child(div().text_xs().text_color(rgb(BRASS)).child(text(
                        desktop.lang,
                        "正在处理",
                        "Working",
                    )))
                }),
        )
        .child(file_list(desktop, cx))
        .child(toggles(desktop, cx))
        .child(actions(desktop.lang, busy, has_items, cx))
}

fn file_list(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let rows = desktop.items.iter().enumerate().map(|(index, item)| {
        let path = item.path.clone();
        let name = item
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.path.display().to_string());
        let mark = item.mark;
        div()
            .id(("row", index))
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
            .child(status_mark(mark, desktop.lang))
            .child(
                div()
                    .id(("remove", index))
                    .text_xs()
                    .text_color(rgb(STONE))
                    .cursor_pointer()
                    .hover(|style| style.text_xs().text_color(rgb(CREAM)))
                    .active(|style| style.text_xs().text_color(rgb(BRASS)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if this.busy {
                            return;
                        }
                        this.items.retain(|item| item.path != path);
                        cx.notify();
                    }))
                    .child(text(desktop.lang, "移出", "Remove")),
            )
    });
    let mut list = div()
        .id("file-list")
        .flex_1()
        .min_h_0()
        .w_full()
        .my(px(LIST_RADIUS))
        .overflow_y_scroll()
        .scrollbar_width(px(14.))
        .track_scroll(&desktop.scroll)
        .flex()
        .flex_col();
    if desktop.items.is_empty() {
        list = list.child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_sm().text_color(rgb(STONE)).child(text(
                    desktop.lang,
                    "拖入文件或文件夹",
                    "Drop files or folders",
                ))),
        );
    } else {
        list = list.children(rows);
    }
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .rounded(px(LIST_RADIUS))
        .bg(rgb(INK))
        .overflow_hidden()
        .flex()
        .flex_col()
        .drag_over::<ExternalPaths>(|style, _paths, _window, _cx| style.bg(rgb(0x242019)))
        .child(list)
        .child(queue_scrollbar(desktop.scroll.clone(), cx))
        .when(desktop.waiting_for_unlock(), |shell| {
            shell.child(wait_mask(desktop.lang))
        })
}

fn status_mark(mark: Mark, lang: Lang) -> impl IntoElement {
    let (label, color, fill_color) = match mark {
        Mark::Checking => (text(lang, "查看", "Check"), STONE, 0x2a261f),
        Mark::Free => (text(lang, "空闲", "Free"), 0x9cba90, 0x243028),
        Mark::Locked => (text(lang, "占用", "Busy"), 0xe7b2a6, 0x3a2420),
        Mark::Critical => (text(lang, "关键", "Critical"), 0xf0d7a2, 0x3a3224),
        Mark::Unlisted => (text(lang, "未知", "Unknown"), BRASS, 0x2a241c),
        Mark::Error => (text(lang, "异常", "Error"), 0xe7b2a6, 0x3a2420),
    };
    div()
        .h(px(22.))
        .px_2()
        .rounded_full()
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_xs()
        .bg(rgb(fill_color))
        .text_color(rgb(color))
        .child(label)
}

fn wait_mask(lang: Lang) -> impl IntoElement {
    div()
        .id("unlock-wait")
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(LIST_RADIUS))
        .bg(gpui::hsla(0., 0., 0., 0.55))
        .on_click(|_, _, _| {})
        .child(
            div()
                .px_5()
                .h(px(40.))
                .rounded_full()
                .bg(rgb(0x242019))
                .flex()
                .items_center()
                .text_sm()
                .child(text(lang, "正在解除占用", "Unlocking")),
        )
}

fn failure_dialog(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let lang = desktop.lang;
    let unlock = desktop.failures.iter().any(|failure| failure.unlock);
    let delete = desktop.failures.iter().any(|failure| !failure.unlock);
    let title = match (unlock, delete) {
        (true, false) => text(lang, "解锁失败", "Unlock failed"),
        (false, true) => text(lang, "删除失败", "Delete failed"),
        _ => text(lang, "没有完成", "Incomplete"),
    };
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::hsla(0., 0., 0., 0.55))
        .child(
            div()
                .w(px(420.))
                .max_h(px(360.))
                .rounded_2xl()
                .bg(rgb(0x242019))
                .p_5()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().child(title))
                .child(
                    div()
                        .id("failure-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(desktop.failures.iter().map(|failure| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_sm().truncate().child(failure.name.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(STONE))
                                        .child(failure.reason.clone()),
                                )
                        })),
                )
                .child(div().flex().justify_end().child(action_button(
                    "failure-close",
                    text(lang, "知道了", "OK"),
                    false,
                    true,
                    cx.listener(|this, _event, _window, cx| {
                        this.failures.clear();
                        cx.notify();
                    }),
                ))),
        )
}

fn scroll_thumb(
    handle: &ScrollHandle,
    track_top: gpui::Pixels,
    travel: gpui::Pixels,
    max: gpui::Pixels,
    cursor_y: gpui::Pixels,
    grab: gpui::Pixels,
) {
    let top = (cursor_y - track_top - grab).clamp(px(0.), travel);
    handle.set_offset(point(px(0.), -max * (top / travel)));
}

fn queue_scrollbar(handle: ScrollHandle, cx: &mut Context<Desktop>) -> impl IntoElement {
    let entity = cx.entity().clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let max = handle.max_offset().height;
            let viewport = handle.bounds().size.height;
            if max <= px(1.) || viewport <= px(1.) || bounds.size.height <= px(1.) {
                return;
            }
            let track = bounds.size.height;
            let thumb_h = (track * (viewport / (viewport + max)))
                .max(px(28.))
                .min(track);
            let travel = (track - thumb_h).max(px(1.));
            let thumb_y = travel * ((-handle.offset().y) / max).clamp(0., 1.);
            let thumb_w = px(4.);
            let thumb_bounds = Bounds::new(
                point(
                    bounds.origin.x + bounds.size.width - thumb_w - px(5.),
                    bounds.origin.y + thumb_y,
                ),
                size(thumb_w, thumb_h),
            );
            let dragging = entity.read(cx).scroll_grab.is_some();
            window.paint_quad(
                fill(thumb_bounds, rgb(if dragging { CREAM } else { STONE })).corner_radii(px(2.)),
            );

            let track_top = bounds.origin.y;
            window.on_mouse_event({
                let entity = entity.clone();
                let handle = handle.clone();
                move |event: &gpui::MouseDownEvent, _, _, cx| {
                    if event.button != gpui::MouseButton::Left || !bounds.contains(&event.position)
                    {
                        return;
                    }
                    let local = event.position.y - track_top;
                    let grab = if local >= thumb_y && local <= thumb_y + thumb_h {
                        local - thumb_y
                    } else {
                        thumb_h * 0.5
                    };
                    scroll_thumb(&handle, track_top, travel, max, event.position.y, grab);
                    entity.update(cx, |this, cx| {
                        this.scroll_grab = Some(grab);
                        cx.notify();
                    });
                }
            });
            window.on_mouse_event({
                let entity = entity.clone();
                let handle = handle.clone();
                move |event: &gpui::MouseMoveEvent, _, _, cx| {
                    if !event.dragging() {
                        return;
                    }
                    let Some(grab) = entity.read(cx).scroll_grab else {
                        return;
                    };
                    scroll_thumb(&handle, track_top, travel, max, event.position.y, grab);
                    cx.notify(entity.entity_id());
                }
            });
            window.on_mouse_event({
                let entity = entity.clone();
                move |_event: &gpui::MouseUpEvent, _, _, cx| {
                    entity.update(cx, |this, cx| {
                        if this.scroll_grab.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .top(px(LIST_RADIUS))
    .bottom(px(LIST_RADIUS))
    .right_0()
    .w(px(18.))
    .cursor_pointer()
}

fn toggles(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    div()
        .flex()
        .gap_3()
        .child(toggle(
            "unlock",
            text(desktop.lang, "先解除占用", "Unlock first"),
            desktop.unlock,
            cx.listener(|this, _event, _window, cx| this.toggle_unlock(cx)),
        ))
        .child(toggle(
            "force",
            text(desktop.lang, "强制结束", "Force quit"),
            desktop.force,
            cx.listener(|this, _event, _window, cx| this.toggle_force(cx)),
        ))
}

fn toggle(
    id: &'static str,
    title: &'static str,
    on: bool,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .h(px(48.))
        .px_4()
        .rounded_2xl()
        .bg(rgb(INK))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x242019)))
        .active(|style| style.bg(rgb(0x2a261f)))
        .on_click(click)
        .child(div().text_sm().child(title))
        .child(switch(on))
}

fn switch(on: bool) -> impl IntoElement {
    let mut track = div()
        .w(px(36.))
        .h(px(20.))
        .rounded_full()
        .px(px(2.))
        .flex()
        .items_center()
        .flex_shrink_0();
    if on {
        track = track.bg(rgb(CREAM)).justify_end();
    } else {
        track = track.bg(rgb(LINE)).justify_start();
    }
    track.child(
        div()
            .size_4()
            .rounded_full()
            .bg(rgb(if on { CANVAS } else { STONE })),
    )
}

fn actions(lang: Lang, busy: bool, has_items: bool, cx: &mut Context<Desktop>) -> impl IntoElement {
    let enabled = has_items && !busy;
    div()
        .flex()
        .justify_end()
        .gap_2()
        .child(action_button(
            "refresh",
            text(lang, "查看占用", "Check locks"),
            false,
            enabled,
            cx.listener(|this, _event, _window, cx| this.run_job(cx, Job::Refresh)),
        ))
        .child(action_button(
            "unlock-btn",
            text(lang, "解除占用", "Unlock"),
            false,
            enabled,
            cx.listener(|this, _event, _window, cx| this.run_job(cx, Job::Unlock)),
        ))
        .child(action_button(
            "delete-btn",
            text(lang, "删除", "Delete"),
            true,
            enabled,
            cx.listener(|this, _event, _window, cx| this.run_job(cx, Job::Delete)),
        ))
}

fn action_button(
    id: &'static str,
    label: &'static str,
    danger: bool,
    enabled: bool,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(id)
        .h(px(36.))
        .px_4()
        .rounded_full()
        .flex()
        .items_center()
        .text_sm()
        .cursor_pointer();
    if danger {
        button = button.bg(rgb(DELETE)).text_color(rgb(CREAM));
    } else {
        button = button
            .border_1()
            .border_color(rgb(LINE))
            .text_color(rgb(CREAM));
    }
    if !enabled {
        button = button.opacity(0.4);
    }
    button
        .hover(move |style| {
            if enabled && !danger {
                style.border_color(rgb(BRASS))
            } else if enabled {
                style.opacity(0.88)
            } else {
                style
            }
        })
        .active(move |style| {
            if enabled && danger {
                style.bg(rgb(DELETE_PRESSED))
            } else if enabled {
                style.bg(rgb(0x2a261f))
            } else {
                style
            }
        })
        .on_click(move |event, window, cx| {
            if enabled {
                click(event, window, cx);
            }
        })
        .child(label)
}

fn settings_page(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    div()
        .id("settings-page")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .px_8()
        .pb_6()
        .gap_4()
        .child(settings_heading(desktop.lang, cx))
        .child(version_card(desktop, cx))
        .child(default_settings(desktop, cx))
        .child(context_menu_section(desktop, cx))
        .child(config_path_line(desktop))
}

fn settings_heading(lang: Lang, cx: &mut Context<Desktop>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(action_button(
            "settings-back",
            text(lang, "返回", "Back"),
            false,
            true,
            cx.listener(|this, _event, _window, cx| {
                this.page = Page::Queue;
                cx.notify();
            }),
        ))
        .child(
            div()
                .text_sm()
                .text_color(rgb(STONE))
                .child(text(lang, "设置", "Settings")),
        )
}

fn version_card(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let lang = desktop.lang;
    div()
        .h(px(48.))
        .flex_shrink_0()
        .px_4()
        .rounded_2xl()
        .bg(rgb(INK))
        .flex()
        .items_center()
        .justify_between()
        .text_sm()
        .line_height(px(20.))
        .child(
            div()
                .text_sm()
                .line_height(px(20.))
                .text_color(rgb(STONE))
                .child(text(lang, "版本", "Version")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .line_height(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(env!("CARGO_PKG_VERSION")),
                )
                .child(version_status(desktop, cx)),
        )
}

fn version_status(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let lang = desktop.lang;
    let open = matches!(desktop.update, UpdateState::Available(_));
    let (caption, color) = match &desktop.update {
        UpdateState::Checking => (text(lang, "正在检查", "Checking").to_string(), STONE),
        UpdateState::Current => (text(lang, "已是最新", "Up to date").to_string(), STONE),
        UpdateState::Available(version) => {
            let caption = match lang {
                Lang::Zh => format!("可更新到 {version}"),
                Lang::En => format!("Update to {version}"),
            };
            (caption, BRASS)
        }
        UpdateState::Installing => (text(lang, "正在更新", "Updating").to_string(), STONE),
        UpdateState::Missing => (
            text(lang, "暂无发布版本", "No release yet").to_string(),
            STONE,
        ),
        UpdateState::Failed => (text(lang, "检查失败", "Check failed").to_string(), 0xe7b2a6),
        UpdateState::UpdateFailed => (
            text(lang, "更新失败", "Update failed").to_string(),
            0xe7b2a6,
        ),
    };
    let mut status = div()
        .id("update-status")
        .text_sm()
        .line_height(px(20.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color));
    if open {
        status = status
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.install_update(cx);
            }));
    }
    status.child(caption)
}

fn default_settings(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let lang = desktop.lang;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(STONE))
                .child(text(lang, "默认", "Defaults")),
        )
        .child(div().text_xs().text_color(rgb(STONE)).child(text(
            lang,
            "写入程序旁边的 ffrm.cfg，下次打开仍然有效。",
            "Saved in ffrm.cfg next to the program, and used next time.",
        )))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .pt_1()
                .child(toggle(
                    "settings-unlock",
                    text(lang, "先解除占用", "Unlock first"),
                    desktop.unlock,
                    cx.listener(|this, _event, _window, cx| this.toggle_unlock(cx)),
                ))
                .child(toggle(
                    "settings-force",
                    text(lang, "强制结束", "Force quit"),
                    desktop.force,
                    cx.listener(|this, _event, _window, cx| this.toggle_force(cx)),
                ))
                .child(toggle(
                    "settings-pin",
                    text(lang, "窗口前置", "Keep in front"),
                    desktop.pinned,
                    cx.listener(|this, _event, window, cx| this.toggle_pin(window, cx)),
                )),
        )
}

fn context_menu_section(desktop: &Desktop, cx: &mut Context<Desktop>) -> impl IntoElement {
    let lang = desktop.lang;
    let mut card = div()
        .rounded_2xl()
        .bg(rgb(INK))
        .p_4()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_sm().child(menu_status(desktop.menu_state, lang)))
        .child(div().text_xs().text_color(rgb(STONE)).child(text(
            lang,
            "右键文件或文件夹会打开窗口并加入队列，不会直接删除。",
            "Right-click a file or folder to queue it here. Nothing is deleted yet.",
        )))
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_button(
                    "menu-add",
                    text(lang, "添加到右键菜单", "Add to context menu"),
                    false,
                    true,
                    cx.listener(|this, _event, _window, cx| this.install_menu(cx)),
                ))
                .child(action_button(
                    "menu-remove",
                    text(lang, "移除右键菜单", "Remove from context menu"),
                    false,
                    true,
                    cx.listener(|this, _event, _window, cx| this.remove_menu(cx)),
                )),
        );
    if let Some(note) = &desktop.menu_note {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(if note.ok { BRASS } else { 0xe7b2a6 }))
                .child(note.text.clone()),
        );
    }
    if desktop.win11 {
        card = card.child(div().text_xs().text_color(rgb(STONE)).child(text(
            lang,
            "Windows 11 要在右键菜单里点「显示更多选项」。",
            "On Windows 11, choose Show more options.",
        )));
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(STONE))
                .child(text(lang, "右键菜单", "Context menu")),
        )
        .child(card)
}

fn menu_status(state: MenuState, lang: Lang) -> &'static str {
    match state {
        MenuState::Absent => text(lang, "未添加到右键菜单", "Not in the context menu"),
        MenuState::Current => text(lang, "已添加到右键菜单", "In the context menu"),
        MenuState::Other => text(
            lang,
            "右键菜单需要重新添加",
            "Add the context menu again to update it",
        ),
    }
}

fn config_path_line(desktop: &Desktop) -> impl IntoElement {
    let mut block = div().flex().flex_col().gap_1().child(
        div()
            .w_full()
            .text_xs()
            .text_color(rgb(STONE))
            .truncate()
            .child(config::config_path().display().to_string()),
    );
    if let Some(error) = &desktop.config_error {
        block = block.child(
            div()
                .text_xs()
                .text_color(rgb(0xe7b2a6))
                .child(error.clone()),
        );
    }
    block
}

pub fn run(initial: Vec<PathBuf>) {
    update::clear_retired();
    let (config, config_error) = Config::load();
    let lang = config.lang;
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1120.), px(630.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(window_title(lang).into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_min_size: Some(size(px(1120.), px(630.))),
                ..Default::default()
            },
            move |window, cx| {
                let pinned = config.pinned;
                window.on_next_frame(move |window, _cx| {
                    apply_window_icon(window);
                    if pinned {
                        set_topmost(window, true);
                    }
                });
                cx.new(move |cx| {
                    let mut desktop = Desktop::from_config(config, config_error);
                    if !initial.is_empty() {
                        desktop.add_paths(&initial, cx);
                    }
                    desktop
                })
            },
        )
        .expect("无法打开窗口");
        cx.activate(true);
    });
}
