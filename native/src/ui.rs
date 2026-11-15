//! Native user interface. All audio and model work belongs on the controller
//! thread; commands and events keep that work off the window/event thread.
use std::{path::PathBuf, sync::mpsc::{self, Receiver, Sender}, time::Duration};

use eframe::egui::{self, ViewportCommand};
use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
#[cfg(windows)]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language { Chinese, English }

impl Language {
    fn text(self, chinese: &'static str, english: &'static str) -> &'static str {
        match self { Self::Chinese => chinese, Self::English => english }
    }
    fn is_english(self) -> bool { self == Self::English }
    fn from_setting(value: &str) -> Option<Self> {
        match value.trim() { "zh-CN" => Some(Self::Chinese), "en" => Some(Self::English), _ => None }
    }
    fn setting(self) -> &'static str { match self { Self::Chinese => "zh-CN", Self::English => "en" } }
    fn title(self) -> &'static str { self.text("指定人声提取", "Voice Isolation") }
}

fn language_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
        .join("VoiceIsolation").join("rust_language_v1.txt")
}

fn language_from_windows_langid(id: u16) -> Language {
    if id & 0x03ff == 0x04 { Language::Chinese } else { Language::English }
}

#[cfg(windows)]
fn system_language() -> Language {
    #[link(name = "kernel32")]
    extern "system" { fn GetUserDefaultUILanguage() -> u16; }
    language_from_windows_langid(unsafe { GetUserDefaultUILanguage() })
}
#[cfg(not(windows))]
fn system_language() -> Language { Language::English }

fn load_language() -> Language {
    std::fs::read_to_string(language_path()).ok().and_then(|s| Language::from_setting(&s)).unwrap_or_else(system_language)
}
pub(crate) fn preferred_english() -> bool { load_language().is_english() }
fn save_language(language: Language) -> std::io::Result<()> {
    let path = language_path();
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, language.setting())
}

#[derive(Clone, Copy)]
enum TrayAction { Show, Quit }

// A hidden winit window need not redraw, so polling tray receivers from
// App::update can leave the tray permanently unresponsive. Wake the actual
// native window from the tray callback before asking egui to process the event.
#[cfg(windows)]
fn wake_hidden_window(hwnd: isize, activate: bool) {
    #[link(name = "user32")]
    extern "system" { fn ShowWindowAsync(hwnd: isize, command: i32) -> i32; }
    // SW_RESTORE for an explicit Show; SW_SHOWNA for Quit avoids stealing focus.
    unsafe { ShowWindowAsync(hwnd, if activate { 9 } else { 8 }); }
}

fn dispatch_tray_action(tx: &Sender<TrayAction>, repaint: &egui::Context, hwnd: Option<isize>, action: TrayAction) {
    let _ = tx.send(action);
    #[cfg(windows)]
    if let Some(hwnd) = hwnd { wake_hidden_window(hwnd, matches!(action, TrayAction::Show)); }
    #[cfg(not(windows))]
    let _ = hwnd;
    repaint.request_repaint();
}

fn install_tray_handlers(cc: &eframe::CreationContext<'_>, tray: &Tray) -> Receiver<TrayAction> {
    let (tx, rx) = mpsc::channel();
    #[cfg(windows)]
    let hwnd = cc.window_handle().ok().and_then(|handle| match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    });
    #[cfg(not(windows))]
    let hwnd = None;
    let show_id = tray.show.id().clone();
    let quit_id = tray.quit.id().clone();
    let repaint = cc.egui_ctx.clone();
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = if event.id == show_id { Some(TrayAction::Show) }
            else if event.id == quit_id { Some(TrayAction::Quit) } else { None };
        if let Some(action) = action {
            dispatch_tray_action(&menu_tx, &repaint, hwnd, action);
        }
    }));
    let repaint = cc.egui_ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if matches!(event, TrayIconEvent::DoubleClick { .. }) {
            dispatch_tray_action(&tx, &repaint, hwnd, TrayAction::Show);
        }
    }));
    rx
}

#[derive(Debug)]
pub enum Command {
    RefreshDevices,
    Record { input: usize, input_name: String, path: PathBuf },
    Start { input: usize, input_name: String, output: usize, output_name: String, reference: PathBuf, strong_music: bool, block_size: usize, use_gpu: bool, gpu_adapter: Option<(u32, u64)> },
    Stop,
    RecordComparison { input: usize, input_name: String, reference: PathBuf, strong_music: bool, block_size: usize, use_gpu: bool, gpu_adapter: Option<(u32, u64)> },
    PlayComparison { output: usize, output_name: String, processed: bool },
    Shutdown,
}

#[derive(Debug)]
pub enum Event {
    Devices { inputs: Vec<String>, outputs: Vec<String>, default_output: Option<String>, gpu_adapters: Vec<(u32, u64, String)>, gpu_error: Option<String> },
    /// Controller reports completion, errors, and running state explicitly.
    Status { message: String, busy: bool, running: bool },
    ComparisonReady,
    ReferenceRecorded(PathBuf),
}

struct Tray {
    _icon: TrayIcon,
    show: MenuItem,
    quit: MenuItem,
}

impl Tray {
    fn new(language: Language) -> Result<Self, Box<dyn std::error::Error>> {
        let menu = Menu::new();
        let show = MenuItem::new(language.text("显示窗口", "Show window"), true, None);
        let quit = MenuItem::new(language.text("退出软件", "Exit"), true, None);
        menu.append(&show)?;
        menu.append(&quit)?;
        // A small self-contained blue microphone icon, no external asset needed.
        let mut pixels = vec![0u8; 32 * 32 * 4];
        for y in 0..32usize { for x in 0..32usize {
            let i = (y * 32 + x) * 4;
            let inside = (x as i32 - 15).pow(2) + (y as i32 - 15).pow(2) <= 225;
            let mic = (12..20).contains(&x) && (7..19).contains(&y)
                || (9..23).contains(&x) && (19..21).contains(&y)
                || (15..17).contains(&x) && (20..25).contains(&y);
            if inside { pixels[i..i+4].copy_from_slice(if mic { &[255,255,255,255] } else { &[38,105,205,255] }); }
        }}
        let icon = Icon::from_rgba(pixels, 32, 32)?;
        let icon = TrayIconBuilder::new().with_tooltip(language.title())
            .with_icon(icon).with_menu(Box::new(menu)).build()?;
        Ok(Self { _icon: icon, show, quit })
    }

    fn set_language(&self, language: Language) {
        self.show.set_text(language.text("显示窗口", "Show window"));
        self.quit.set_text(language.text("退出软件", "Exit"));
        let _ = self._icon.set_tooltip(Some(language.title()));
    }
}

struct App {
    commands: Sender<Command>, events: Receiver<Event>, tray: Option<Tray>,
    tray_actions: Option<Receiver<TrayAction>>,
    language: Language,
    inputs: Vec<String>, outputs: Vec<String>, input: usize, output: usize,
    comparison_output: usize, reference: PathBuf, strong_music: bool, block_size: usize, use_gpu: bool,
    gpu_adapters: Vec<(u32, u64, String)>, gpu_adapter: Option<(u32, u64)>, gpu_error: Option<String>,
    status: String, busy: bool, running: bool, comparison_ready: bool,
    hidden: bool, exiting: bool,
}

fn default_reference() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir).join("VoiceIsolation").join("我的声样.wav")
}

fn processing_mode_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir).join("VoiceIsolation").join("rust_processing_mode_v1.txt")
}

fn load_processing_mode() -> usize {
    match std::fs::read_to_string(processing_mode_path()).ok().as_deref().map(str::trim) {
        Some("160") => crate::audio::LOW_CPU_BLOCK_SIZE,
        Some("240") => crate::audio::LOWEST_CPU_BLOCK_SIZE,
        _ => crate::audio::BLOCK_SIZE,
    }
}

fn save_processing_mode(block_size: usize) -> std::io::Result<()> {
    let path = processing_mode_path();
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, (block_size * 1000 / crate::audio::SAMPLE_RATE as usize).to_string())
}

impl App {
    fn comparison_source(&self) -> (Option<String>, PathBuf, bool, bool, usize, Option<(u32, u64)>) {
        (self.inputs.get(self.input).cloned(), self.reference.clone(), self.strong_music, self.use_gpu, self.block_size, self.gpu_adapter)
    }

    fn gpu_ready(&self) -> bool {
        gpu_selection_ready(self.use_gpu, &self.gpu_adapters, self.gpu_adapter, self.gpu_error.is_some())
    }

    fn new(cc: &eframe::CreationContext<'_>, commands: Sender<Command>, events: Receiver<Event>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        // Windows installs one of these fonts in ordinary installations.
        for path in [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"] {
            if let Ok(bytes) = std::fs::read(path) {
                fonts.font_data.insert("chinese".into(), egui::FontData::from_owned(bytes).into());
                fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "chinese".into());
                cc.egui_ctx.set_fonts(fonts);
                break;
            }
        }
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.override_text_color = Some(egui::Color32::from_rgb(31, 43, 60));
        style.visuals.panel_fill = egui::Color32::from_rgb(245, 247, 251);
        style.visuals.window_fill = egui::Color32::WHITE;
        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(244, 247, 251);
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(242, 245, 250);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(229, 237, 250);
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(46, 105, 207);
        style.spacing.item_spacing = egui::vec2(9.0, 9.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
        cc.egui_ctx.set_style(style);
        let language = load_language();
        let tray = Tray::new(language).ok();
        let tray_actions = tray.as_ref().map(|tray| install_tray_handlers(cc, tray));
        let reference = default_reference();
        let _ = commands.send(Command::RefreshDevices);
        Self { commands, events, tray, tray_actions, language, inputs: vec![], outputs: vec![], input: 0, output: 0,
            comparison_output: usize::MAX, reference, strong_music: false, block_size: load_processing_mode(), use_gpu: false,
            gpu_adapters: vec![], gpu_adapter: None, gpu_error: None,
            status: "正在读取音频设备…".into(), busy: true, running: false,
            comparison_ready: false, hidden: false, exiting: false }
    }

    fn send(&mut self, command: Command, message: &str) {
        if self.commands.send(command).is_ok() { self.busy = true; self.status = message.into(); }
        else { self.status = "后台处理线程已退出，请重新启动程序。".into(); }
    }
    fn change_language(&mut self, ctx: &egui::Context, language: Language) {
        self.language = language;
        if let Some(tray) = &self.tray { tray.set_language(language); }
        ctx.send_viewport_cmd(ViewportCommand::Title(language.title().to_owned()));
        if let Err(error) = save_language(language) {
            self.status = format!("界面语言已切换，但无法保存设置：{error}");
        }
    }
    fn exit(&mut self, ctx: &egui::Context) {
        self.exiting = true;
        let _ = self.commands.send(Command::Shutdown);
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    fn show(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
    fn hide(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() {
            self.hidden = true;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        } else { self.exit(ctx); }
    }
}

fn abbreviated(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars { return text.to_owned(); }
    format!("{}…", text.chars().take(max_chars.saturating_sub(1)).collect::<String>())
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(egui::Color32::WHITE)
        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(225, 231, 240)))
        .corner_radius(10).inner_margin(egui::Margin::symmetric(16, 13))
        .show(ui, |ui| { ui.set_width(ui.available_width()); add(ui); });
}

fn title(ui: &mut egui::Ui, text: &str, detail: &str) {
    ui.label(egui::RichText::new(text).size(17.0).strong());
    ui.label(egui::RichText::new(detail).size(13.0).color(egui::Color32::from_rgb(104, 119, 140)));
    ui.add_space(3.0);
}

fn device_combo(ui: &mut egui::Ui, id: &str, label: &str, devices: &[String], selected: &mut usize, language: Language) {
    ui.label(egui::RichText::new(label).strong());
    let full = devices.get(*selected).map(String::as_str).unwrap_or(if devices.is_empty() { language.text("没有可用设备", "No devices available") } else { language.text("请选择设备", "Select a device") });
    let full = crate::i18n::render_status(full, language.is_english());
    let limit = (ui.available_width() / 13.0).clamp(18.0, 48.0) as usize;
    egui::ComboBox::from_id_salt(id)
        .selected_text(abbreviated(&full, limit))
        .width((ui.available_width() - 20.0).clamp(150.0, 530.0)).show_ui(ui, |ui| {
                for (i, name) in devices.iter().enumerate() { ui.selectable_value(selected, i, crate::i18n::render_status(name, language.is_english())); }
        }).response.on_hover_text(full);
}

fn likely_virtual_output(name: &str) -> bool {
    let n = name.to_lowercase();
    ["cable", "vb-audio", "vb audio", "voicemeeter", "virtual", "虚拟", "obs audio", "steam streaming"]
        .iter().any(|marker| n.contains(marker))
}

fn gpu_selection_ready(use_gpu: bool, adapters: &[(u32, u64, String)], selected: Option<(u32, u64)>, has_error: bool) -> bool {
    if !use_gpu { return true; }
    if has_error || adapters.is_empty() { return false; }
    selected.map_or(true, |(index, luid)| adapters.iter().any(|(i, l, _)| *i == index && *l == luid))
}

fn remap_gpu_selection(selected: Option<(u32, u64)>, adapters: &[(u32, u64, String)]) -> Option<(u32, u64)> {
    selected.map(|(old_index, old_luid)| {
        adapters.iter().find(|(_, luid, _)| *luid == old_luid)
            .map(|(index, luid, _)| (*index, *luid)).unwrap_or((old_index, old_luid))
    })
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let previous_comparison_source = self.comparison_source();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Devices { inputs, outputs, default_output, gpu_adapters, gpu_error } => {
                    let previous_input = self.inputs.get(self.input).cloned();
                    let previous_output = self.outputs.get(self.output).cloned();
                    let previous_comparison = self.outputs.get(self.comparison_output).cloned();
                    self.inputs = inputs; self.outputs = outputs;
                    self.input = previous_input.and_then(|name| self.inputs.iter().position(|s| *s == name)).unwrap_or(0);
                    self.output = previous_output.and_then(|name| self.outputs.iter().position(|s| *s == name))
                        .or_else(|| self.outputs.iter().position(|s| s.to_lowercase().contains("cable input"))).unwrap_or(0);
                    self.comparison_output = previous_comparison.and_then(|name| self.outputs.iter().position(|s| *s == name))
                        .or_else(|| default_output.as_ref().and_then(|name| self.outputs.iter().position(|s| s == name && !likely_virtual_output(s))))
                        .or_else(|| self.outputs.iter().position(|s| !likely_virtual_output(s))).unwrap_or(usize::MAX);
                    self.gpu_adapter = remap_gpu_selection(self.gpu_adapter, &gpu_adapters);
                    self.gpu_adapters = gpu_adapters;
                    self.gpu_error = gpu_error;
                    self.busy = false; self.status = "设备列表已刷新。".into();
                }
                Event::Status { message, busy, running } => { self.status = message; self.busy = busy; self.running = running; }
                Event::ComparisonReady => { self.comparison_ready = true; self.busy = false; self.status = "试听已准备好。".into(); }
                Event::ReferenceRecorded(path) => { self.reference = path; self.comparison_ready = false; self.busy = false; self.status = "声样录制完成。".into(); }
            }
        }
        while let Some(action) = self.tray_actions.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match action { TrayAction::Show => self.show(ctx), TrayAction::Quit => self.exit(ctx) }
        }
        if !self.exiting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        // If the tray could not be created, leave the window normally
        // minimized on the taskbar instead of quitting unexpectedly.
        if self.tray.is_some() && !self.hidden && !self.exiting && ctx.input(|i| i.viewport().minimized == Some(true)) { self.hide(ctx); }
        ctx.request_repaint_after(Duration::from_millis(100));
        let previous_language = self.language;
        egui::TopBottomPanel::top("language_header").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(previous_language.title()).size(22.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("ui_language")
                        .selected_text(match self.language { Language::Chinese => "简体中文", Language::English => "English" })
                        .width(110.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.language, Language::Chinese, "简体中文");
                            ui.selectable_value(&mut self.language, Language::English, "English");
                        });
                    ui.label("语言 / Language");
                });
            });
        });
        if self.language != previous_language { self.change_language(ctx, self.language); }
        let language = self.language;
        // Keep status and transport visible while the settings scroll.
        egui::TopBottomPanel::bottom("transport")
            .frame(egui::Frame::new().fill(egui::Color32::WHITE)
                .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(225, 231, 240)))
                .inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ctx, |ui| {
                let needs_attention = ["失败", "异常", "不可用", "无法", "丢块", "空缓冲", "错误"]
                    .iter().any(|word| self.status.contains(word));
                let (state, color) = if self.busy { (language.text("处理中", "Working"), egui::Color32::from_rgb(168, 112, 22)) }
                    else if needs_attention { (language.text("需要注意", "Attention"), egui::Color32::from_rgb(180, 75, 54)) }
                    else if self.running { (language.text("运行中", "Running"), egui::Color32::from_rgb(22, 135, 99)) }
                    else { (language.text("已停止", "Stopped"), egui::Color32::from_rgb(104, 119, 140)) };
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("●").color(color));
                    ui.label(egui::RichText::new(state).color(color).strong());
                    ui.label(egui::RichText::new(crate::i18n::render_status(&self.status, language.is_english())).size(13.0));
                });
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    let can_start = !self.busy && !self.running && !self.inputs.is_empty() && !self.outputs.is_empty() && self.gpu_ready();
                    let start = egui::Button::new(egui::RichText::new(language.text("开始处理", "Start processing")).strong().color(if can_start { egui::Color32::WHITE } else { egui::Color32::from_rgb(135, 146, 163) }))
                        .fill(if can_start { egui::Color32::from_rgb(45, 105, 208) } else { egui::Color32::from_rgb(231, 235, 241) }).min_size(egui::vec2(110.0, 34.0));
                    if ui.add_enabled(can_start, start).clicked() {
                        let message = if self.use_gpu { "正在验证显卡并加载模型，可能需要十几秒…" } else { "正在加载模型…" };
                        self.send(Command::Start { input: self.input, input_name: self.inputs[self.input].clone(), output: self.output, output_name: self.outputs[self.output].clone(), reference: self.reference.clone(), strong_music: self.strong_music, block_size: self.block_size, use_gpu: self.use_gpu, gpu_adapter: self.gpu_adapter }, message);
                    }
                    if ui.add_enabled(!self.busy && self.running, egui::Button::new(language.text("停止处理", "Stop processing")).min_size(egui::vec2(94.0, 34.0))).clicked() { self.send(Command::Stop, "正在停止…"); }
                    if ui.button(language.text("退出软件", "Exit")).clicked() { self.exit(ctx); }
                    ui.label(egui::RichText::new(if self.tray.is_some() { language.text("关闭窗口后在托盘运行", "Closing the window keeps the app in the tray") } else { language.text("关闭窗口将退出", "Closing the window exits") }).size(12.0).color(egui::Color32::from_rgb(119, 132, 150)));
                });
            });
        egui::CentralPanel::default().show(ctx, |ui| { egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(language.text("游戏语音中减少音乐和视频背景声。", "Reduce music and video background sound in game chat."))
                .size(14.0).color(egui::Color32::from_rgb(104, 119, 140)));
            ui.add_space(9.0);
            ui.add_enabled_ui(!self.busy && !self.running, |ui| {
                card(ui, |ui| {
                title(ui, language.text("音频设备", "Audio devices"), language.text("选择实体麦克风，将处理后的声音输出到虚拟声卡。", "Select a physical microphone and send processed audio to a virtual cable."));
                device_combo(ui, "input_device", language.text("输入 · 麦克风", "Input · Microphone"), &self.inputs, &mut self.input, language);
                ui.add_space(3.0);
                device_combo(ui, "output_device", language.text("输出 · 虚拟声卡", "Output · Virtual cable"), &self.outputs, &mut self.output, language);
                if ui.button(language.text("刷新设备", "Refresh devices")).clicked() { self.send(Command::RefreshDevices, "正在刷新设备…"); }
                });
                ui.add_space(10.0);
                card(ui, |ui| {
                title(ui, language.text("我的声样", "Voice sample"), language.text("录制一段自己的声音，或选择已有的 WAV 声样。", "Record your voice or choose an existing WAV sample."));
                let path_text = self.reference.to_string_lossy();
                let name = if language.is_english() && self.reference == default_reference() {
                    "Recorded voice sample (WAV)".to_owned()
                } else {
                    self.reference.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| language.text("未选择声样", "No sample selected").into())
                };
                ui.label(egui::RichText::new(format!("{} {}", language.text("当前声样：", "Current sample:"), abbreviated(&name, 38))).strong()).on_hover_text(path_text);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.inputs.is_empty(), egui::Button::new(language.text("录入声音（12 秒）", "Record voice (12 s)"))).clicked() {
                        self.send(Command::Record { input: self.input, input_name: self.inputs[self.input].clone(), path: default_reference() }, "录音中，请连续说话 12 秒…");
                    }
                    if ui.button(language.text("选择已有声样", "Choose existing sample")).clicked() {
                        if let Some(path) = rfd::FileDialog::new().set_title(language.text("选择已有声样", "Choose voice sample")).add_filter(language.text("WAV 音频", "WAV audio"), &["wav"]).pick_file() { self.reference = path; }
                    }
                });
                });
                ui.add_space(10.0);
                card(ui, |ui| {
                title(ui, language.text("处理设置", "Processing settings"), language.text("在响应速度与 CPU 占用之间选择适合当前场景的模式。", "Choose a mode that balances responsiveness and CPU use."));
                ui.checkbox(&mut self.strong_music, language.text("加强背景音乐过滤（启动处理或试听时生效）", "Stronger background music filtering (applied on start or preview)"));
                ui.checkbox(&mut self.use_gpu, language.text("使用显卡加速声纹提取（降噪仍由 CPU 处理）", "Use GPU for speaker extraction (denoising still uses CPU)"));
                if self.use_gpu {
                    let selected_text = match self.gpu_adapter {
                        None => language.text("自动选择（按显存优先）", "Automatic (prefer dedicated GPU memory)").to_string(),
                        Some((index, luid)) => self.gpu_adapters.iter().find(|(i, l, _)| *i == index && *l == luid)
                            .map(|(_, _, name)| format!("{name} ({} {index}, ID {luid:016X})", language.text("设备", "device")))
                            .unwrap_or_else(|| language.text("所选显卡已不可用，请重新选择", "Selected GPU unavailable; choose again").to_string()),
                    };
                    ui.label(egui::RichText::new(language.text("显卡设备", "GPU device")).strong());
                        egui::ComboBox::from_id_salt("gpu_adapter")
                            .selected_text(abbreviated(&selected_text, (ui.available_width() / 13.0).clamp(18.0, 48.0) as usize)).width((ui.available_width() - 20.0).clamp(150.0, 530.0)).show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.gpu_adapter, None, language.text("自动选择（按显存优先）", "Automatic (prefer dedicated GPU memory)"));
                                for (index, luid, name) in &self.gpu_adapters {
                                    ui.selectable_value(&mut self.gpu_adapter, Some((*index, *luid)), format!("{name} ({} {index}, ID {luid:016X})", language.text("设备", "device")));
                                }
                            }).response.on_hover_text(selected_text);
                    if let Some(error) = &self.gpu_error { ui.label(format!("{} {}", language.text("显卡列表不可用：", "GPU list unavailable:"), crate::i18n::render_status(error, language.is_english()))); }
                    else if self.gpu_adapters.is_empty() { ui.label(language.text("没有发现可用硬件显卡，请检查驱动并刷新设备。", "No hardware GPU found. Check the driver and refresh devices.")); }
                    else if !self.gpu_ready() { ui.label(language.text("所选显卡已不可用，请刷新或重新选择。", "The selected GPU is unavailable. Refresh or select another.")); }
                    ui.label(language.text("建议先选择“更省 CPU”；显卡繁忙时可能断音。", "Try Lowest CPU first; a busy GPU may cause audio gaps."));
                }
                ui.label(egui::RichText::new(language.text("处理模式", "Processing mode")).strong());
                    let previous_block_size = self.block_size;
                    egui::ComboBox::from_id_salt("processing_block_size")
                        .selected_text(match self.block_size {
                            crate::audio::LOW_CPU_BLOCK_SIZE => language.text("省 CPU（160 毫秒音频块）", "Lower CPU (160 ms blocks)"),
                            crate::audio::LOWEST_CPU_BLOCK_SIZE => language.text("更省 CPU（240 毫秒音频块）", "Lowest CPU (240 ms blocks)"),
                            _ => language.text("低延迟（80 毫秒音频块）", "Low latency (80 ms blocks)"),
                        }).width((ui.available_width() - 20.0).clamp(150.0, 530.0)).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.block_size, crate::audio::BLOCK_SIZE, language.text("低延迟（80 毫秒音频块）", "Low latency (80 ms blocks)"));
                            ui.selectable_value(&mut self.block_size, crate::audio::LOW_CPU_BLOCK_SIZE, language.text("省 CPU（160 毫秒音频块）", "Lower CPU (160 ms blocks)"));
                            ui.selectable_value(&mut self.block_size, crate::audio::LOWEST_CPU_BLOCK_SIZE, language.text("更省 CPU（240 毫秒音频块）", "Lowest CPU (240 ms blocks)"));
                        });
                    let reserve = match self.block_size { crate::audio::LOW_CPU_BLOCK_SIZE => 40, crate::audio::LOWEST_CPU_BLOCK_SIZE => 80, _ => 20 };
                    ui.label(egui::RichText::new(if language.is_english() { format!("Playback buffering adds approximately {reserve} ms of delay.") } else { format!("抗抖缓冲额外增加约 {reserve} 毫秒等待。") }).size(13.0));
                    if self.block_size != previous_block_size {
                        if let Err(error) = save_processing_mode(self.block_size) {
                            self.status = format!("处理模式已切换，但无法保存设置：{error}");
                        }
                    }
                });
            });
            // A preview belongs to the exact input, enrollment and processing
            // settings it was recorded with. Never label an old CPU preview as
            // the result of a newly selected GPU (or other) configuration.
            if self.comparison_source() != previous_comparison_source { self.comparison_ready = false; }
            ui.add_space(10.0);
            card(ui, |ui| {
            title(ui, language.text("效果试听", "Preview"), language.text("先停止处理，录制 5 秒，再通过实体耳机或扬声器对比。", "Stop processing, record five seconds, then compare through headphones or speakers."));
            if self.comparison_output == usize::MAX && !self.outputs.is_empty() {
                ui.label(language.text("未找到明显的实体播放设备，请手动选择试听设备。", "No physical playback device found automatically. Select one for preview."));
            }
            ui.add_enabled_ui(!self.busy && !self.running, |ui| {
                device_combo(ui, "preview_device", language.text("试听设备", "Preview device"), &self.outputs, &mut self.comparison_output, language);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.inputs.is_empty() && self.gpu_ready(), egui::Button::new(language.text("录制 5 秒试听", "Record 5-second preview"))).clicked() {
                        self.comparison_ready = false;
                        self.send(Command::RecordComparison { input: self.input, input_name: self.inputs[self.input].clone(), reference: self.reference.clone(), strong_music: self.strong_music, block_size: self.block_size, use_gpu: self.use_gpu, gpu_adapter: self.gpu_adapter }, "正在录制并处理试听…");
                    }
                    if ui.add_enabled(self.comparison_ready && self.comparison_output < self.outputs.len(), egui::Button::new(language.text("播放原声", "Play original"))).clicked() {
                        self.send(Command::PlayComparison { output: self.comparison_output, output_name: self.outputs[self.comparison_output].clone(), processed: false }, "正在播放原声…");
                    }
                    if ui.add_enabled(self.comparison_ready && self.comparison_output < self.outputs.len(), egui::Button::new(language.text("播放处理后", "Play processed"))).clicked() {
                        self.send(Command::PlayComparison { output: self.comparison_output, output_name: self.outputs[self.comparison_output].clone(), processed: true }, "正在播放处理结果…");
                    }
                });
            });
            });
            ui.add_space(14.0);
        }); });
    }
}

/// Run on the main thread: Windows tray icons require a window message loop.
pub fn run(commands: Sender<Command>, events: Receiver<Event>) -> eframe::Result {
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default()
        .with_inner_size([720.0, 760.0]).with_min_inner_size([520.0, 420.0]), ..Default::default() };
    eframe::run_native(load_language().title(), options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, commands, events)))))
}

#[cfg(test)]
mod tests {
    use super::{gpu_selection_ready, remap_gpu_selection, language_from_windows_langid, Language};

    #[test]
    fn language_preference_round_trips_and_system_default_is_language_specific() {
        for language in [Language::Chinese, Language::English] {
            assert_eq!(Language::from_setting(language.setting()), Some(language));
        }
        assert_eq!(Language::from_setting("not-a-language"), None);
        assert_eq!(language_from_windows_langid(0x0804), Language::Chinese); // Simplified Chinese
        assert_eq!(language_from_windows_langid(0x0404), Language::Chinese); // Traditional Chinese
        assert_eq!(language_from_windows_langid(0x0409), Language::English); // English
        assert_eq!(language_from_windows_langid(0x0411), Language::English); // Japanese
        assert_eq!(language_from_windows_langid(0x0412), Language::English); // Korean
    }

    #[test]
    fn gpu_selection_survives_reordering_but_not_removal_or_error() {
        let devices = vec![(0, 0x1111, "Same GPU".to_string()), (3, 0x2222, "Same GPU".to_string())];
        assert!(gpu_selection_ready(false, &[], Some((7, 7)), true));
        assert!(!gpu_selection_ready(true, &[], None, false));
        assert!(!gpu_selection_ready(true, &devices, None, true));
        assert!(gpu_selection_ready(true, &devices, None, false));
        assert!(gpu_selection_ready(true, &devices, Some((3, 0x2222)), false));
        assert!(!gpu_selection_ready(true, &devices, Some((0, 0x2222)), false));
        assert_eq!(remap_gpu_selection(Some((7, 0x2222)), &devices), Some((3, 0x2222)));
        assert_eq!(remap_gpu_selection(Some((7, 0x3333)), &devices), Some((7, 0x3333)));
        assert!(!gpu_selection_ready(true, &devices, Some((7, 0x3333)), false));
    }
}
