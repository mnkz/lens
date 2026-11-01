#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod gdi;
mod hotkey;
mod ocr;
mod overlay;
mod pin;
mod pixels;
mod store;
mod ui;
mod util;
mod view;
mod web;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, IconData, ViewportBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::capture::Shot;
use crate::hotkey::Kind;
use crate::ui::{BG, STROKE, TEXT};
use crate::util::{log_line, panic_message, wide};

const WIN_W: f32 = 400.0;
const WIN_H: f32 = 640.0;

static MUTEX: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static BUSY: AtomicBool = AtomicBool::new(false);

fn main() {
    capture::ensure_dpi();
    if !claim_instance() {
        message_box("Lens", "Lens est déjà dans la zone de notification.");
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("panic: {info}"));
    }));
    let freeze = std::env::args().skip(1).any(|arg| arg == "--freeze");
    let err = eframe::run_native(
        "lens",
        native_options(),
        Box::new(move |cc| {
            let mut app = LensApp::new(cc);
            if freeze {
                app.queued = Some(Kind::Freeze);
            }
            Ok(Box::new(app))
        }),
    );
    hotkey::remove();
    pin::close_all();
    match err {
        Ok(()) => log_line("fenêtre fermée"),
        Err(err) => {
            let msg = format!("Lens n'a pas pu démarrer.\n{err}");
            log_line(&msg);
            message_box("Lens", &msg);
        }
    }
}

enum Cmd {
    Toggle(Option<tray_icon::Rect>),
    Open,
    Quit,
    Freeze,
}

enum Bg {
    History(Vec<store::Thumb>),
    Ocr {
        generation: u64,
        text: String,
        err: Option<String>,
    },
}

pub(crate) struct Current {
    bgra: Vec<u8>,
    w: i32,
    h: i32,
    png: Vec<u8>,
    text: String,
    selection: String,
}

pub(crate) struct ThumbView {
    path: PathBuf,
    tex: egui::TextureHandle,
}

pub struct LensApp {
    tray: Option<TrayIcon>,
    rx: Receiver<Cmd>,
    bg_rx: Receiver<Bg>,
    bg_tx: Sender<Bg>,
    visible: bool,
    placed: bool,
    start_in_tray: bool,
    tray_hold: u8,
    quitting: bool,
    icon_rect: Option<tray_icon::Rect>,
    queued: Option<Kind>,
    defer: Option<Kind>,
    current: Option<Current>,
    shot_tex: Option<egui::TextureHandle>,
    thumbs: Vec<ThumbView>,
    status: String,
    ocr_gen: u64,
    startup: bool,
    shown_at: Instant,
    had_focus: bool,
    lens_page: Option<PathBuf>,
    lens_slot: Option<egui::Rect>,
    lens_view: Option<view::Pane>,
}

impl LensApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_look(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let (bg_tx, bg_rx) = mpsc::channel();
        let tray = install_tray(tx, cc.egui_ctx.clone());
        hotkey::install(cc.egui_ctx.clone());
        let start_in_tray = launched_in_tray();
        let tx_hist = bg_tx.clone();
        let ctx = cc.egui_ctx.clone();
        std::thread::Builder::new()
            .name("lens-history".into())
            .spawn(move || {
                let thumbs = store::load_thumbs(8);
                let _ = tx_hist.send(Bg::History(thumbs));
                ctx.request_repaint();
            })
            .ok();
        Self {
            tray,
            rx,
            bg_rx,
            bg_tx,
            visible: !start_in_tray,
            placed: false,
            start_in_tray,
            tray_hold: 0,
            quitting: false,
            icon_rect: None,
            queued: None,
            defer: None,
            current: None,
            shot_tex: None,
            thumbs: Vec::new(),
            status: String::new(),
            ocr_gen: 0,
            startup: store::startup_enabled(),
            shown_at: Instant::now(),
            had_focus: false,
            lens_page: None,
            lens_slot: None,
            lens_view: None,
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.had_focus = false;
        if let Some(view) = &mut self.lens_view {
            view.hide();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.start_in_tray = false;
        self.visible = true;
        self.shown_at = Instant::now();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.place(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn place(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point().max(1.0);
        let area = work_area(ppp);
        let mut size = ctx
            .input(|input| input.viewport().inner_rect.map(|rect| rect.size()))
            .unwrap_or(egui::vec2(WIN_W, WIN_H));
        if size.x < 200.0 {
            size.x = WIN_W;
        }
        let max_h = (area.height() - 16.0).max(420.0);
        if self.lens_page.is_some() {
            let lens_h = 860.0_f32.min(max_h);
            if size.y < lens_h - 1.0 {
                size.y = lens_h;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            }
        } else if size.y > max_h || size.y < 200.0 {
            size.y = max_h.clamp(420.0, WIN_H);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        let (edge, area) = taskbar_dock(ppp).unwrap_or((BarEdge::Bottom, area));
        let gap = 8.0;
        let (mut x, mut y) = match edge {
            BarEdge::Top => (area.right() - size.x - gap, area.top() + gap),
            BarEdge::Bottom => (area.right() - size.x - gap, area.bottom() - size.y - gap),
            BarEdge::Left => (area.left() + gap, area.bottom() - size.y - gap),
            BarEdge::Right => (area.right() - size.x - gap, area.bottom() - size.y - gap),
        };
        if let Some(icon) = self.icon_rect.filter(rect_usable) {
            let left = icon.position.x as f32 / ppp;
            let right = left + icon.size.width as f32 / ppp;
            let top = icon.position.y as f32 / ppp;
            let bottom = top + icon.size.height as f32 / ppp;
            match edge {
                BarEdge::Top => {
                    x = right - size.x;
                    y = bottom + gap;
                }
                BarEdge::Bottom => {
                    x = right - size.x;
                    y = top - size.y - gap;
                }
                BarEdge::Left => {
                    x = right + gap;
                    y = bottom - size.y;
                }
                BarEdge::Right => {
                    x = left - size.x - gap;
                    y = bottom - size.y;
                }
            }
        }
        x = x.clamp(
            area.left() + 4.0,
            (area.right() - size.x - 4.0).max(area.left() + 4.0),
        );
        y = y.clamp(
            area.top() + 4.0,
            (area.bottom() - size.y - 4.0).max(area.top() + 4.0),
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
        self.placed = true;
    }

    fn dismiss_outside(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.visible {
            self.had_focus = false;
            return;
        }
        if self.shown_at.elapsed() <= Duration::from_millis(200) {
            return;
        }
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        let inside = pointer_in_lens(frame) || foreground_is_lens(frame);
        if self.had_focus && !focused && !inside && !self.cursor_on_tray() {
            self.hide(ctx);
            return;
        }
        if focused || inside {
            self.had_focus = true;
        }
    }

    fn cursor_on_tray(&self) -> bool {
        let Some(icon) = &self.icon_rect else {
            return false;
        };
        let mut pt = windows_sys::Win32::Foundation::POINT::default();
        unsafe {
            if windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) == 0 {
                return false;
            }
        }
        let x = pt.x as f64;
        let y = pt.y as f64;
        x >= icon.position.x
            && x < icon.position.x + icon.size.width as f64
            && y >= icon.position.y
            && y < icon.position.y + icon.size.height as f64
    }

    fn handle_commands(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Cmd::Quit => {
                    self.lens_view.take();
                    self.quitting = true;
                    self.tray.take();
                    pin::close_all();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Cmd::Open => self.show(ctx),
                Cmd::Freeze => self.queued = Some(Kind::Freeze),
                Cmd::Toggle(rect) => {
                    if let Some(rect) = rect {
                        self.icon_rect = Some(rect);
                    }
                    if self.visible {
                        self.hide(ctx);
                    } else {
                        self.show(ctx);
                    }
                }
            }
        }
    }

    fn poll_bg(&mut self, ctx: &egui::Context) {
        let mut changed = false;
        while let Ok(msg) = self.bg_rx.try_recv() {
            changed = true;
            match msg {
                Bg::History(thumbs) => {
                    self.thumbs = thumbs
                        .into_iter()
                        .map(|thumb| ThumbView {
                            path: thumb.path,
                            tex: rgba_texture(ctx, &thumb.rgba, thumb.w, thumb.h),
                        })
                        .collect();
                }
                Bg::Ocr {
                    generation,
                    text,
                    err,
                } => {
                    if generation == self.ocr_gen
                        && let Some(current) = &mut self.current
                    {
                        current.text = text;
                        if current.selection.is_empty()
                            && current.text.is_empty()
                            && let Some(err) = err
                        {
                            self.status = err;
                        }
                    }
                }
            }
        }
        if changed {
            ctx.request_repaint();
        }
    }

    fn run_capture(&mut self, ctx: &egui::Context, mut kind: Kind) {
        BUSY.store(true, Ordering::SeqCst);
        let app = self as *mut LensApp;
        let ctx = ctx.clone();
        let _guard = BusyGuard;
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let app = unsafe { &mut *app };
            loop {
                let shot = match capture::capture() {
                    Ok(shot) => shot,
                    Err(err) => {
                        app.status = err;
                        app.show(&ctx);
                        return;
                    }
                };
                if kind == Kind::Window {
                    let hwnd = hotkey::take_target();
                    if let Some(grab) = window_grab(&shot, hwnd) {
                        app.accept(&ctx, grab);
                        return;
                    }
                    kind = Kind::Freeze;
                }
                match overlay::run(shot) {
                    overlay::End::Grab(grab) => {
                        app.accept(&ctx, grab);
                        return;
                    }
                    overlay::End::Restart => {
                        kind = hotkey::take().unwrap_or(Kind::Freeze);
                    }
                    overlay::End::Cancel => {
                        if let Some(next) = hotkey::take() {
                            kind = next;
                            continue;
                        }
                        return;
                    }
                }
            }
        }));
        if let Err(payload) = result {
            log_line(&format!("capture {}", panic_message(payload.as_ref())));
            self.status = "La capture a échoué.".into();
            self.show(&ctx);
        }
    }

    fn accept(&mut self, ctx: &egui::Context, grab: overlay::Grab) {
        self.lens_page = None;
        let png = match pixels::encode_png(&grab.bgra, grab.width, grab.height) {
            Ok(png) => png,
            Err(err) => {
                self.status = err;
                self.show(ctx);
                return;
            }
        };
        self.status = match clipboard::copy_image(&grab.bgra, grab.width, grab.height, &png) {
            Ok(()) => "Dans le presse-papiers".into(),
            Err(err) => err,
        };
        let path = store::keep(&png);
        if grab.action == overlay::CloseAction::Save {
            match store::save_screenshots(&png) {
                Ok(saved) => self.status = format!("Enregistré · {}", saved.display()),
                Err(err) => self.status = err,
            }
        }
        if grab.action == overlay::CloseAction::Pin {
            pin::open(
                &grab.bgra,
                grab.width,
                grab.height,
                grab.screen_x,
                grab.screen_y,
            );
        }
        let selection = grab.selection.clone();
        let text = grab.text.clone();
        self.current = Some(Current {
            bgra: grab.bgra,
            w: grab.width,
            h: grab.height,
            png,
            text,
            selection: selection.clone(),
        });
        self.reload_shot(ctx);
        self.push_thumb(ctx, path);
        if grab.action == overlay::CloseAction::Web {
            let query = if selection.is_empty() {
                self.current
                    .as_ref()
                    .map(|cur| cur.text.clone())
                    .unwrap_or_default()
            } else {
                selection
            };
            self.status = if query.is_empty() {
                "Pas de texte".into()
            } else {
                match web::google_text(&query) {
                    Ok(()) => "Recherche ouverte · image dans le presse-papiers".into(),
                    Err(err) => err,
                }
            };
        }
        if grab.action == overlay::CloseAction::Lens {
            self.launch_lens(ctx);
        }
        if self.current.as_ref().is_some_and(|cur| cur.text.is_empty()) {
            self.launch_ocr(ctx);
        }
        self.show(ctx);
    }

    fn reload_shot(&mut self, ctx: &egui::Context) {
        let Some(current) = &self.current else {
            self.shot_tex = None;
            return;
        };
        let rgba = pixels::to_rgba(&current.bgra);
        self.shot_tex = Some(rgba_texture(ctx, &rgba, current.w as u32, current.h as u32));
    }

    fn push_thumb(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(current) = &self.current else {
            return;
        };
        let (rgba, w, h) = pixels::thumb_rgba(&current.bgra, current.w, current.h, 160);
        self.thumbs.insert(
            0,
            ThumbView {
                path,
                tex: rgba_texture(ctx, &rgba, w, h),
            },
        );
        self.thumbs.truncate(8);
    }

    pub(crate) fn copy_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        self.status = match clipboard::copy_image(&current.bgra, current.w, current.h, &current.png)
        {
            Ok(()) => "Dans le presse-papiers".into(),
            Err(err) => err,
        };
    }

    pub(crate) fn copy_text_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let text = if current.selection.is_empty() {
            current.text.clone()
        } else {
            current.selection.clone()
        };
        if text.is_empty() {
            self.status = "Pas encore de texte".into();
            return;
        }
        self.status = match clipboard::copy_text(&text) {
            Ok(()) => "Texte copié".into(),
            Err(err) => err,
        };
    }

    pub(crate) fn launch_lens(&mut self, ctx: &egui::Context) {
        let Some(current) = &self.current else {
            return;
        };
        if let Err(err) = clipboard::copy_image(&current.bgra, current.w, current.h, &current.png) {
            self.status = err;
            return;
        }
        let png = current.png.clone();
        match web::lens_page(&png) {
            Ok(path) => {
                self.lens_page = Some(path);
                self.status = "Lens · image dans le presse-papiers".into();
                self.placed = false;
                self.place(ctx);
            }
            Err(err) => self.status = err,
        }
        ctx.request_repaint();
    }

    fn place_lens(&mut self, frame: &eframe::Frame, ctx: &egui::Context) {
        let show = self.visible && self.lens_page.is_some() && self.lens_slot.is_some();
        if !show {
            if let Some(view) = &mut self.lens_view {
                view.hide();
            }
            return;
        }
        let page = self.lens_page.clone().expect("page");
        let slot = self.lens_slot.expect("slot");
        let ppp = ctx.pixels_per_point().max(1.0);
        let x = (slot.min.x * ppp).round().max(0.0) as i32;
        let y = (slot.min.y * ppp).round().max(0.0) as i32;
        let w = (slot.width() * ppp).round().max(1.0) as u32;
        let h = (slot.height() * ppp).round().max(1.0) as u32;
        if h < 48 || w < 48 {
            if let Some(view) = &mut self.lens_view {
                view.hide();
            }
            return;
        }
        if self.lens_view.is_none() {
            match view::Pane::open(frame, &page, x, y, w, h) {
                Ok(pane) => self.lens_view = Some(pane),
                Err(err) => {
                    log_line(&format!("webview {err}"));
                    self.lens_page = None;
                    self.status = "Google Lens indisponible dans la fenêtre.".into();
                }
            }
            return;
        }
        if let Some(pane) = &mut self.lens_view
            && let Err(err) = pane.sync(&page, x, y, w, h)
        {
            log_line(&format!("webview {err}"));
        }
    }

    pub(crate) fn save_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let _ = clipboard::copy_image(&current.bgra, current.w, current.h, &current.png);
        self.status = match store::save_screenshots(&current.png) {
            Ok(path) => format!("Enregistré · {}", path.display()),
            Err(err) => err,
        };
    }

    pub(crate) fn pin_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let (x, y) = capture::cursor();
        pin::open(&current.bgra, current.w, current.h, x, y);
        let _ = clipboard::copy_image(&current.bgra, current.w, current.h, &current.png);
        self.status = "Épinglé · image dans le presse-papiers".into();
    }

    pub(crate) fn web_current(&mut self) {
        let Some(current) = &self.current else {
            return;
        };
        let query = if current.selection.is_empty() {
            current.text.clone()
        } else {
            current.selection.clone()
        };
        if query.is_empty() {
            self.status = "Pas de texte".into();
            return;
        }
        let _ = clipboard::copy_image(&current.bgra, current.w, current.h, &current.png);
        self.status = match web::google_text(&query) {
            Ok(()) => "Recherche ouverte · image dans le presse-papiers".into(),
            Err(err) => err,
        };
    }

    pub(crate) fn open_folder(&mut self) {
        let dir = store::screenshots_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.status = match web::open_path(&dir) {
            Ok(()) => "Dossier ouvert".into(),
            Err(err) => err,
        };
    }

    pub(crate) fn toggle_startup(&mut self) {
        let next = !self.startup;
        self.status = match store::set_startup(next) {
            Ok(()) => {
                self.startup = store::startup_enabled();
                if self.startup {
                    "Ajouté au démarrage".into()
                } else {
                    "Retiré du démarrage".into()
                }
            }
            Err(err) => err,
        };
    }

    pub(crate) fn open_thumb(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Ok(bytes) = std::fs::read(&path) else {
            self.status = "Image introuvable".into();
            return;
        };
        let Ok((bgra, w, h)) = pixels::decode_png(&bytes) else {
            self.status = "Image illisible".into();
            return;
        };
        self.status = match clipboard::copy_image(&bgra, w, h, &bytes) {
            Ok(()) => "Dans le presse-papiers".into(),
            Err(err) => err,
        };
        self.lens_page = None;
        self.current = Some(Current {
            bgra,
            w,
            h,
            png: bytes,
            text: String::new(),
            selection: String::new(),
        });
        self.reload_shot(ctx);
        self.launch_ocr(ctx);
        self.show(ctx);
    }

    fn launch_ocr(&mut self, ctx: &egui::Context) {
        let Some(current) = &self.current else {
            return;
        };
        self.ocr_gen = self.ocr_gen.wrapping_add(1);
        let generation = self.ocr_gen;
        let bgra = current.bgra.clone();
        let w = current.w;
        let h = current.h;
        let tx = self.bg_tx.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("lens-ocr".into())
            .spawn(move || {
                let (text, err) = match ocr::recognize(&bgra, w, h) {
                    Ok(words) if words.is_empty() => (String::new(), None),
                    Ok(words) => (ocr::join_words(&words, 0, words.len() - 1), None),
                    Err(err) => {
                        log_line(&format!("ocr {err}"));
                        (String::new(), Some(err))
                    }
                };
                let _ = tx.send(Bg::Ocr {
                    generation,
                    text,
                    err,
                });
                ctx.request_repaint();
            })
            .ok();
    }
}

struct BusyGuard;

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

fn window_grab(shot: &Shot, hwnd: windows_sys::Win32::Foundation::HWND) -> Option<overlay::Grab> {
    let rect = capture::window_image_rect(hwnd, shot)?;
    let bgra = pixels::crop(&shot.bgra, shot.width, rect)?;
    Some(overlay::Grab {
        bgra,
        width: rect.w,
        height: rect.h,
        action: overlay::CloseAction::Copy,
        text: String::new(),
        selection: String::new(),
        screen_x: rect.x + shot.origin_x,
        screen_y: rect.y + shot.origin_y,
    })
}

fn rgba_texture(ctx: &egui::Context, rgba: &[u8], w: u32, h: u32) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba);
    ctx.load_texture(
        format!("lens-{w}x{h}-{}", rgba.len()),
        image,
        egui::TextureOptions::LINEAR,
    )
}

impl eframe::App for LensApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if BUSY.load(Ordering::SeqCst) {
            return;
        }
        self.handle_commands(ctx);
        if self.quitting {
            return;
        }
        if self.start_in_tray && self.tray_hold < 4 {
            self.tray_hold += 1;
            self.hide(ctx);
        }
        if !self.placed {
            self.place(ctx);
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        if self.visible
            && self.shown_at.elapsed() > Duration::from_millis(200)
            && ctx.input(|input| input.key_pressed(egui::Key::Escape))
        {
            self.hide(ctx);
        }
        self.dismiss_outside(ctx, frame);
        self.poll_bg(ctx);
        if let Some(kind) = hotkey::take() {
            self.queued = Some(kind);
        }
        if let Some(kind) = self.defer.take() {
            std::thread::sleep(Duration::from_millis(50));
            self.run_capture(ctx, kind);
            return;
        }
        if let Some(kind) = self.queued.take() {
            if self.visible {
                self.hide(ctx);
                self.defer = Some(kind);
                ctx.request_repaint();
            } else {
                self.run_capture(ctx, kind);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui);
        self.place_lens(frame, ui.ctx());
    }
}

fn install_tray(tx: Sender<Cmd>, ctx: egui::Context) -> Option<TrayIcon> {
    let tx_click = tx.clone();
    let ctx_click = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = event
        {
            let _ = tx_click.send(Cmd::Toggle(Some(rect)));
            ctx_click.request_repaint();
        }
    }));
    let tx_menu = tx;
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == "quit" {
            let _ = tx_menu.send(Cmd::Quit);
        } else if event.id == "open" {
            let _ = tx_menu.send(Cmd::Open);
        } else if event.id == "freeze" {
            let _ = tx_menu.send(Cmd::Freeze);
        }
        ctx.request_repaint();
    }));
    let open = Box::leak(Box::new(MenuItem::with_id("open", "Ouvrir", true, None)));
    let freeze = Box::leak(Box::new(MenuItem::with_id(
        "freeze",
        "Figer les écrans",
        true,
        None,
    )));
    let quit = Box::leak(Box::new(MenuItem::with_id("quit", "Quitter", true, None)));
    let menu = Menu::new();
    if menu.append(open).is_err() || menu.append(freeze).is_err() || menu.append(quit).is_err() {
        log_line("menu indisponible");
    }
    let icon = tray_icon::Icon::from_rgba(lens_mark(32), 32, 32).ok()?;
    match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon)
        .with_tooltip("Lens")
        .build()
    {
        Ok(tray) => Some(tray),
        Err(err) => {
            log_line(&format!("icône {err}"));
            None
        }
    }
}

fn install_look(ctx: &egui::Context) {
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        let mut fonts = FontDefinitions::default();
        fonts
            .font_data
            .insert("Segoe".to_string(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "Segoe".to_string());
        }
        ctx.set_fonts(fonts);
    }
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            style.visuals.dark_mode = true;
            style.visuals.panel_fill = BG;
            style.visuals.window_fill = BG;
            style.visuals.extreme_bg_color = BG;
            style.visuals.faint_bg_color = gray(12);
            style.visuals.code_bg_color = BG;
            style.visuals.override_text_color = Some(TEXT);
            style.visuals.hyperlink_color = TEXT;
            style.visuals.window_stroke = egui::Stroke::new(1.0, STROKE);
            style.visuals.window_corner_radius = egui::CornerRadius::ZERO;
            style.visuals.menu_corner_radius = egui::CornerRadius::ZERO;
            style.visuals.window_shadow = egui::epaint::Shadow::NONE;
            style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
            style.visuals.selection.bg_fill = TEXT;
            style.visuals.selection.stroke.color = BG;
            paint_widget(&mut style.visuals.widgets.noninteractive);
            paint_widget(&mut style.visuals.widgets.inactive);
            paint_widget(&mut style.visuals.widgets.hovered);
            paint_widget(&mut style.visuals.widgets.active);
            paint_widget(&mut style.visuals.widgets.open);
            style.visuals.widgets.noninteractive.bg_fill = BG;
            style.visuals.widgets.inactive.bg_fill = BG;
            style.visuals.widgets.hovered.bg_fill = gray(18);
            style.visuals.widgets.active.bg_fill = TEXT;
            style.visuals.widgets.active.fg_stroke.color = BG;
            style.visuals.widgets.open.bg_fill = gray(18);
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.scroll.bar_width = 4.0;
            style.animation_time = 0.08;
        });
    }
}

fn paint_widget(visual: &mut egui::style::WidgetVisuals) {
    visual.corner_radius = egui::CornerRadius::same(2);
    visual.bg_stroke = egui::Stroke::new(1.0, STROKE);
    visual.fg_stroke = egui::Stroke::new(1.0, TEXT);
}

fn gray(v: u8) -> egui::Color32 {
    egui::Color32::from_rgb(v, v, v)
}

fn native_options() -> eframe::NativeOptions {
    let mark = lens_mark(64);
    let icon = IconData {
        rgba: mark,
        width: 64,
        height: 64,
    };
    eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Lens")
            .with_inner_size([WIN_W, WIN_H])
            .with_min_inner_size([360.0, 420.0])
            .with_decorations(false)
            .with_transparent(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_visible(!launched_in_tray())
            .with_resizable(true)
            .with_active(!launched_in_tray())
            .with_icon(Arc::new(icon)),
        centered: false,
        run_and_return: true,
        ..Default::default()
    }
}

fn launched_in_tray() -> bool {
    std::env::args().skip(1).any(|arg| arg == "--tray")
}

fn lens_mark(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let cx = s * 0.40;
    let cy = s * 0.40;
    let rad = s * 0.22;
    let thick = (s * 0.075).max(1.6);
    let start = rad + thick * 0.35;
    let ax = cx + start * std::f32::consts::FRAC_1_SQRT_2;
    let ay = cy + start * std::f32::consts::FRAC_1_SQRT_2;
    let bx = s * 0.84;
    let by = s * 0.84;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let ring = (dist - rad).abs() <= thick * 0.5;
            let handle = dist_to_seg(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by) <= thick * 0.5;
            if ring || handle {
                let i = ((y * size + x) * 4) as usize;
                px[i] = 244;
                px[i + 1] = 244;
                px[i + 2] = 242;
                px[i + 3] = 255;
            }
        }
    }
    px
}

fn dist_to_seg(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let len2 = abx * abx + aby * aby;
    if len2 < 0.01 {
        return (px - ax).hypot(py - ay);
    }
    let t = ((px - ax) * abx + (py - ay) * aby) / len2;
    let t = t.clamp(0.0, 1.0);
    (px - (ax + abx * t)).hypot(py - (ay + aby * t))
}

#[derive(Clone, Copy)]
enum BarEdge {
    Left,
    Top,
    Right,
    Bottom,
}

fn taskbar_dock(ppp: f32) -> Option<(BarEdge, egui::Rect)> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    use windows_sys::Win32::UI::Shell::{
        ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA, SHAppBarMessage,
    };
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..APPBARDATA::default()
    };
    if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) } == 0 {
        return None;
    }
    let edge = match data.uEdge {
        ABE_TOP => BarEdge::Top,
        ABE_LEFT => BarEdge::Left,
        ABE_RIGHT => BarEdge::Right,
        _ => BarEdge::Bottom,
    };
    let monitor = unsafe { MonitorFromRect(&data.rc, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let scale = ppp.max(1.0);
    let mut area = egui::Rect::from_min_max(
        egui::pos2(
            info.rcWork.left as f32 / scale,
            info.rcWork.top as f32 / scale,
        ),
        egui::pos2(
            info.rcWork.right as f32 / scale,
            info.rcWork.bottom as f32 / scale,
        ),
    );
    match edge {
        BarEdge::Top if info.rcWork.top <= info.rcMonitor.top => {
            area.min.y += (data.rc.bottom - data.rc.top).max(0) as f32 / scale;
        }
        BarEdge::Bottom if info.rcWork.bottom >= info.rcMonitor.bottom => {
            area.max.y -= (data.rc.bottom - data.rc.top).max(0) as f32 / scale;
        }
        BarEdge::Left if info.rcWork.left <= info.rcMonitor.left => {
            area.min.x += (data.rc.right - data.rc.left).max(0) as f32 / scale;
        }
        BarEdge::Right if info.rcWork.right >= info.rcMonitor.right => {
            area.max.x -= (data.rc.right - data.rc.left).max(0) as f32 / scale;
        }
        _ => {}
    }
    if area.width() < 100.0 || area.height() < 100.0 {
        None
    } else {
        Some((edge, area))
    }
}

fn pointer_in_lens(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, IsChild, WindowFromPoint};
    let Ok(ours) = view::frame_hwnd(frame) else {
        return true;
    };
    let mut pt = POINT::default();
    unsafe {
        if GetCursorPos(&mut pt) == 0 {
            return true;
        }
        let hit = WindowFromPoint(pt);
        !hit.is_null() && (hit == ours || IsChild(ours, hit) != 0)
    }
}

fn foreground_is_lens(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsChild};
    let Ok(ours) = view::frame_hwnd(frame) else {
        return true;
    };
    let fg = unsafe { GetForegroundWindow() };
    !fg.is_null() && (fg == ours || unsafe { IsChild(ours, fg) } != 0)
}

fn rect_usable(rect: &tray_icon::Rect) -> bool {
    rect.size.width > 0
        && rect.size.height > 0
        && (rect.position.x != 0.0 || rect.position.y != 0.0)
}

fn work_area(ppp: f32) -> egui::Rect {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};
    let mut rect = RECT::default();
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut RECT as *mut _, 0);
    }
    let scale = ppp.max(1.0);
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
    }
    egui::Rect::from_min_max(
        egui::pos2(rect.left as f32 / scale, rect.top as f32 / scale),
        egui::pos2(rect.right as f32 / scale, rect.bottom as f32 / scale),
    )
}

fn claim_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide("Local\\Lens");
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return false;
        }
        MUTEX.store(handle, Ordering::Relaxed);
        true
    }
}

fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    let title = wide(title);
    let text = wide(text);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
