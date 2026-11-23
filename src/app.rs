use std::{error::Error, sync::mpsc};

use eframe::egui::{self, ViewportBuilder, ViewportCommand};
use raw_window_handle::HasWindowHandle;
use rpanel::gesture::DEFAULT_HOLD_MS;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
};

use crate::{
    platform::{
        self, MouseEvent, MouseListener, Popup, Selection, SelectionRequest, SelectionResult,
        Trigger,
    },
    ui::{editor::Editors, panel},
};

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run() -> AppResult<()> {
    let Some(launch) = launch_options()? else {
        return Ok(());
    };
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        clipboard_shortcuts: false,
        viewport: ViewportBuilder::default()
            .with_inner_size(panel::SIZE.map(|dimension| dimension as f32))
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_visible(false)
            .with_taskbar(false),
        ..Default::default()
    };
    platform::configure(&mut options);
    eframe::run_native(
        "RPanel",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, launch)?))),
    )
    .map_err(|error| error.to_string().into())
}

struct LaunchOptions {
    threshold_ms: u32,
    preview: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            threshold_ms: DEFAULT_HOLD_MS,
            preview: false,
        }
    }
}

fn launch_options() -> AppResult<Option<LaunchOptions>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => Ok(Some(LaunchOptions::default())),
        Some("--help" | "-h") if args.next().is_none() => {
            println!("RPanel [--hold-ms <毫秒>]，右键长按阈值默认为 {DEFAULT_HOLD_MS} ms。");
            println!("RPanel --preview，启动时立即预览面板。");
            Ok(None)
        }
        Some("--preview") if args.next().is_none() => Ok(Some(LaunchOptions {
            preview: true,
            ..Default::default()
        })),
        Some("--hold-ms") => {
            let value = args.next().and_then(|value| value.parse::<u32>().ok());
            if let Some(value) = value.filter(|value| *value > 0)
                && args.next().is_none()
            {
                return Ok(Some(LaunchOptions {
                    threshold_ms: value,
                    preview: false,
                }));
            }
            Err("--hold-ms 必须为正整数，使用 --help 查看用法".into())
        }
        _ => Err("参数无效，使用 --help 查看用法".into()),
    }
}

enum AppEvent {
    Mouse(MouseEvent),
    Preview,
    Paste,
    Toggle,
    Exit,
}

struct PanelSelection {
    request: SelectionRequest,
    open_on_ready: bool,
}

struct App {
    events: mpsc::Receiver<AppEvent>,
    listener: MouseListener,
    popup: Popup,
    toggle: CheckMenuItem,
    _tray: TrayIcon,
    listening: bool,
    visible: bool,
    generation: u64,
    selection: Selection,
    panel_selection: Option<PanelSelection>,
    editors: Editors,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, launch: LaunchOptions) -> AppResult<Self> {
        panel::configure(&cc.egui_ctx);
        let threshold_ms = launch.threshold_ms;
        let popup = Popup::attach(cc.window_handle()?.as_raw())?;
        let (sender, events) = mpsc::sync_channel(16);
        let mouse_sender = sender.clone();
        let mouse_ctx = cc.egui_ctx.clone();
        let listener = MouseListener::start(threshold_ms, move |event| {
            send_event(&mouse_sender, &mouse_ctx, AppEvent::Mouse(event));
        })?;
        let selection_ctx = cc.egui_ctx.clone();
        let selection =
            Selection::new(move || selection_ctx.request_repaint_of(egui::ViewportId::ROOT));

        let toggle = CheckMenuItem::new("监听右键长按", true, false, None);
        let exit = MenuItem::new("退出 RPanel", true, None);
        let paste = MenuItem::new("手动粘贴文本", true, None);
        let menu = Menu::new();
        let preview_id = {
            let preview = MenuItem::new("预览「查看Json」", true, None);
            menu.append(&preview)?;
            preview.id().clone()
        };
        menu.append_items(&[&paste, &toggle, &exit])?;
        let tray = TrayIconBuilder::new()
            .with_icon(tray_icon()?)
            .with_tooltip("RPanel")
            .with_menu(Box::new(menu))
            .build()?;
        let toggle_id = toggle.id().clone();
        let paste_id = paste.id().clone();
        let exit_id = exit.id().clone();
        let tray_ctx = cc.egui_ctx.clone();
        if launch.preview {
            send_event(&sender, &tray_ctx, AppEvent::Preview);
        }
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == preview_id {
                send_event(&sender, &tray_ctx, AppEvent::Preview);
            } else if event.id == paste_id {
                send_event(&sender, &tray_ctx, AppEvent::Paste);
            } else if event.id == toggle_id {
                send_event(&sender, &tray_ctx, AppEvent::Toggle);
            } else if event.id == exit_id {
                send_event(&sender, &tray_ctx, AppEvent::Exit);
            }
        }));
        log::info!("RPanel 交互原型已启动，右键长按阈值 {threshold_ms} ms");
        Ok(Self {
            events,
            listener,
            popup,
            toggle,
            _tray: tray,
            listening: false,
            visible: false,
            generation: 0,
            selection,
            panel_selection: None,
            editors: Editors::default(),
        })
    }

    fn show(&mut self, ctx: &egui::Context, trigger: Trigger) {
        self.generation = self.generation.wrapping_add(1);
        self.panel_selection = None;
        let mut request = None;
        match self.popup.show(trigger, panel::SIZE, self.generation, || {
            request = Some(self.selection.begin(trigger));
        }) {
            Ok(Some(bounds)) => {
                self.panel_selection = request.map(|request| PanelSelection {
                    request,
                    open_on_ready: false,
                });
                self.listener.set_panel_bounds(Some(bounds));
                self.visible = true;
                ctx.request_repaint();
            }
            Ok(None) => self.hide(),
            Err(error) => {
                self.listener.set_panel_bounds(None);
                self.popup.hide();
                self.visible = false;
                log::warn!("显示悬浮菜单失败：{error}");
            }
        }
    }

    fn hide(&mut self) {
        self.panel_selection = None;
        if self.visible {
            self.listener.set_panel_bounds(None);
            self.popup.hide();
            self.visible = false;
        }
    }

    fn update_selection(&mut self, ctx: &egui::Context) {
        let Some(selection) = &mut self.panel_selection else {
            return;
        };
        if let Some(remaining) = selection.request.poll() {
            ctx.request_repaint_after(remaining);
        } else if selection.open_on_ready
            && let Some(selection) = self.panel_selection.take()
        {
            let result = selection.request.finish();
            self.hide();
            self.open_editor(ctx, result);
        }
    }

    fn open_editor(&mut self, ctx: &egui::Context, result: SelectionResult) {
        let (text, status) = match result {
            Ok(text) => {
                let status = match self.popup.clipboard().write(&text) {
                    Ok(()) => "已将选中原文复制到剪贴板。",
                    Err(error) => {
                        log::warn!("复制选区原文失败：{error}");
                        "无法复制到剪贴板，选中原文已载入。"
                    }
                };
                (text, status)
            }
            Err(reason) => (String::new(), reason),
        };
        self.editors.open(ctx, text, status);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                AppEvent::Mouse(MouseEvent::Show(trigger)) if self.listening => {
                    self.show(ctx, trigger)
                }
                AppEvent::Preview => {
                    if let Some(trigger) = Popup::preview() {
                        self.show(ctx, trigger);
                    }
                }
                AppEvent::Paste => {
                    self.hide();
                    self.open_editor(ctx, Err("可从剪贴板手动粘贴文本。"));
                }
                AppEvent::Mouse(MouseEvent::Dismiss(generation))
                    if generation == self.generation =>
                {
                    self.hide()
                }
                AppEvent::Mouse(MouseEvent::Listening(enabled)) => {
                    self.listening = enabled;
                    self.toggle.set_checked(enabled);
                    if !enabled {
                        self.hide();
                    }
                }
                AppEvent::Toggle => {
                    if let Err(error) = self.listener.set_enabled(!self.listening) {
                        self.toggle.set_checked(self.listening);
                        log::warn!("无法切换全局监听：{error}");
                    }
                }
                AppEvent::Exit => {
                    self.hide();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                    return;
                }
                _ => {}
            }
        }
        self.update_selection(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(selection) = &mut self.panel_selection
            && panel::show(ui, selection.open_on_ready)
        {
            selection.open_on_ready = true;
            ui.ctx().request_repaint();
            self.update_selection(ui.ctx());
        }
        self.editors.show(ui.ctx(), self.popup.clipboard());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if cfg!(windows) { [1.0; 4] } else { [0.0; 4] }
    }
}

fn send_event(sender: &mpsc::SyncSender<AppEvent>, ctx: &egui::Context, event: AppEvent) {
    match sender.try_send(event) {
        Ok(()) => ctx.request_repaint_of(egui::ViewportId::ROOT),
        Err(mpsc::TrySendError::Full(_)) => log::warn!("GUI 事件队列已满，跳过本次事件"),
        Err(mpsc::TrySendError::Disconnected(_)) => {}
    }
}

fn tray_icon() -> AppResult<Icon> {
    const SIZE: u32 = 16;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let color =
                if (4..=11).contains(&y) && (x == 3 + y.abs_diff(7) || x == 12 - y.abs_diff(7)) {
                    [255, 255, 255, 255]
                } else if (y == 0 || y == SIZE - 1) && (x == 0 || x == SIZE - 1) {
                    [0, 0, 0, 0]
                } else {
                    [0, 92, 185, 255]
                };
            rgba.extend(color);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
