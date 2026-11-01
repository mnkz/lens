use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_OVER, AlphaBlend, BLENDFUNCTION, BS_SOLID, BeginPaint, BitBlt, ClientToScreen,
    CreateFontW, CreatePen, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint, ExtCreatePen, FillRect,
    FrameRect, GdiFlush, GetDC, GetStockObject, GetTextExtentPoint32W, HALFTONE, HBRUSH, HDC,
    HFONT, HOLLOW_BRUSH, InvalidateRect, LOGBRUSH, MONITOR_DEFAULTTONEAREST, MonitorFromPoint,
    NULL_PEN, PS_ENDCAP_ROUND, PS_GEOMETRIC, PS_JOIN_ROUND, PS_SOLID, Polyline, ReleaseDC, SRCCOPY,
    ScreenToClient, SelectObject, SetBkMode, SetBrushOrgEx, SetStretchBltMode, SetTextColor,
    StretchBlt, TRANSPARENT, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::HiDpi::GetDpiForMonitor;
use windows_sys::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F4,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetForegroundWindow, GetMessageW, GetWindowLongPtrW,
    GetWindowThreadProcessId, IDC_ARROW, IDC_CROSS, IDC_IBEAM, LoadCursorW, MSG, PostMessageW,
    PostQuitMessage, RegisterClassExW, SW_SHOW, SWP_SHOWWINDOW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WM_APP, WM_CHAR, WM_CLOSE,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSKEYDOWN, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::capture::{self, Shot};
use crate::clipboard;
use crate::gdi::{self, Dib};
use crate::hotkey;
use crate::ocr::{self, Word};
use crate::pixels::{self, PxRect};
use crate::util::{log_line, panic_message, wide};

pub enum End {
    Cancel,
    Restart,
    Grab(Grab),
}

pub struct Grab {
    pub bgra: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub action: CloseAction,
    pub text: String,
    pub selection: String,
    pub screen_x: i32,
    pub screen_y: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    Copy,
    Lens,
    Save,
    Pin,
    Web,
}

#[derive(Clone, Copy)]
enum HitCmd {
    Close(CloseAction),
    Ocr,
}

const WM_OCR: u32 = WM_APP + 1;

struct OcrDone {
    token: u64,
    result: Result<Vec<Word>, String>,
}

#[derive(Clone, Copy)]
struct Hit {
    rect: RECT,
    cmd: HitCmd,
    label: &'static str,
    primary: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PenKind {
    Color(usize),
    Width(usize),
}

struct PenHit {
    rect: RECT,
    kind: PenKind,
}

const PEN_COLORS: [Rgb; 4] = [
    Rgb {
        r: 208,
        g: 48,
        b: 42,
    },
    Rgb {
        r: 232,
        g: 186,
        b: 48,
    },
    Rgb {
        r: 244,
        g: 244,
        b: 242,
    },
    Rgb {
        r: 18,
        g: 18,
        b: 18,
    },
];

const PEN_WIDTHS: [f32; 3] = [2.0, 4.0, 8.0];

struct Stroke {
    pts: Vec<(i32, i32)>,
    width: i32,
    color: Rgb,
}

struct Overlay {
    hwnd: HWND,
    shot_w: i32,
    shot_h: i32,
    origin_x: i32,
    origin_y: i32,
    bgra: Vec<u8>,
    src: Dib,
    veil: Dib,
    back: Dib,
    stamp: Dib,
    black: HBRUSH,
    white: HBRUSH,
    stroke: HBRUSH,
    gray: HBRUSH,
    hover: HBRUSH,
    pale: HBRUSH,
    font: HFONT,
    font_px: i32,
    sel: Option<PxRect>,
    anchor: Option<(i32, i32)>,
    dragged: bool,
    words: Vec<Word>,
    text_anchor: Option<usize>,
    text_current: Option<usize>,
    text_drag: bool,
    buttons: Vec<Hit>,
    button_arm: Option<usize>,
    drawing: bool,
    draw_arm: bool,
    draw_chips: Vec<RECT>,
    pen_color: usize,
    pen_width: usize,
    pen_hits: Vec<PenHit>,
    pen_arm: Option<PenKind>,
    strokes: Vec<Stroke>,
    pen: Option<Stroke>,
    monitors: Vec<PxRect>,
    status: String,
    ocr_please: bool,
    ocr_rx: Option<Receiver<OcrDone>>,
    ocr_token: u64,
    pending_web: bool,
    done: bool,
    restart: bool,
    outcome: Option<Grab>,
}

pub fn run(shot: Shot) -> End {
    hotkey::set_inside(true);
    let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_inner(shot)));
    hotkey::set_inside(false);
    match end {
        Ok(end) => end,
        Err(payload) => {
            log_line(&format!("overlay {}", panic_message(payload.as_ref())));
            End::Cancel
        }
    }
}

fn run_inner(shot: Shot) -> End {
    let screen = unsafe { windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut()) };
    if screen.is_null() {
        return End::Cancel;
    }
    let veil_px = pixels::veiled(&shot.bgra);
    let src = match Dib::new(screen, &shot.bgra, shot.width, shot.height) {
        Ok(dib) => dib,
        Err(err) => {
            unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen) };
            log_line(&err);
            return End::Cancel;
        }
    };
    let veil = match Dib::new(screen, &veil_px, shot.width, shot.height) {
        Ok(dib) => dib,
        Err(err) => {
            unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen) };
            log_line(&err);
            return End::Cancel;
        }
    };
    let back = match Dib::new(screen, &veil_px, shot.width, shot.height) {
        Ok(dib) => dib,
        Err(err) => {
            unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen) };
            log_line(&err);
            return End::Cancel;
        }
    };
    let stamp = match Dib::new(screen, &[255, 255, 255, 255], 1, 1) {
        Ok(dib) => dib,
        Err(err) => {
            unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen) };
            log_line(&err);
            return End::Cancel;
        }
    };
    unsafe { windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen) };
    let monitors = capture::monitor_rects(shot.origin_x, shot.origin_y);
    let overlay = Box::new(Overlay {
        hwnd: std::ptr::null_mut(),
        shot_w: shot.width,
        shot_h: shot.height,
        origin_x: shot.origin_x,
        origin_y: shot.origin_y,
        bgra: shot.bgra,
        src,
        veil,
        back,
        stamp,
        black: brush(0, 0, 0),
        white: brush(244, 244, 242),
        stroke: brush(42, 42, 42),
        gray: brush(138, 138, 136),
        hover: brush(18, 18, 18),
        pale: brush(186, 206, 220),
        font: std::ptr::null_mut(),
        font_px: 0,
        sel: None,
        anchor: None,
        dragged: false,
        words: Vec::new(),
        text_anchor: None,
        text_current: None,
        text_drag: false,
        buttons: Vec::new(),
        button_arm: None,
        drawing: false,
        draw_arm: false,
        draw_chips: Vec::new(),
        pen_color: 0,
        pen_width: 1,
        pen_hits: Vec::new(),
        pen_arm: None,
        strokes: Vec::new(),
        pen: None,
        monitors,
        status: String::new(),
        ocr_please: false,
        ocr_rx: None,
        ocr_token: 0,
        pending_web: false,
        done: false,
        restart: false,
        outcome: None,
    });
    register_class();
    let state = Box::into_raw(overlay);
    let title = wide("Lens");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            (*state).origin_x,
            (*state).origin_y,
            (*state).shot_w,
            (*state).shot_h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            state as *mut _,
        )
    };
    if hwnd.is_null() {
        log_line("fenêtre de gel impossible");
        drop(unsafe { Box::from_raw(state) });
        return End::Cancel;
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetWindowPos(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
        );
        focus(hwnd);
        UpdateWindow(hwnd);
    }
    loop {
        let mut msg = MSG::default();
        let pulled = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if pulled == 0 {
            unsafe { PostQuitMessage(msg.wParam as i32) };
            break;
        }
        if pulled == -1 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let overlay = unsafe { &mut *state };
        if overlay.ocr_please {
            overlay.ocr_please = false;
            overlay.start_ocr();
        }
        if overlay.done {
            break;
        }
        if hotkey::peek() {
            overlay.restart = true;
            break;
        }
    }
    let (end, hwnd) = {
        let overlay = unsafe { &mut *state };
        let end = if overlay.restart {
            End::Restart
        } else if let Some(grab) = overlay.outcome.take() {
            End::Grab(grab)
        } else {
            End::Cancel
        };
        let hwnd = overlay.hwnd;
        overlay.hwnd = std::ptr::null_mut();
        (end, hwnd)
    };
    if !hwnd.is_null() {
        unsafe { DestroyWindow(hwnd) };
    }
    drop(unsafe { Box::from_raw(state) });
    end
}

impl Overlay {
    fn clamped(&self) -> Option<PxRect> {
        self.sel?.clamp_bounds(self.shot_w, self.shot_h)
    }

    fn cursor(&self) -> (i32, i32) {
        let mut pt = POINT::default();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            ScreenToClient(self.hwnd, &mut pt);
        }
        (pt.x, pt.y)
    }

    fn scale_at(&self, x: i32, y: i32) -> f32 {
        let mut pt = POINT { x, y };
        unsafe {
            ClientToScreen(self.hwnd, &mut pt);
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut dx = 96u32;
            let mut dy = 96u32;
            if !monitor.is_null() {
                let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            }
            (dx.max(96) as f32) / 96.0
        }
    }

    fn font_for(&mut self, hdc: HDC, scale: f32) -> HFONT {
        let px = (15.0 * scale).round().clamp(13.0, 40.0) as i32;
        if self.font_px == px && !self.font.is_null() {
            return self.font;
        }
        unsafe {
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
            let name = wide("Segoe UI");
            self.font = CreateFontW(-px, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, name.as_ptr());
            let _ = hdc;
            self.font_px = px;
            self.font
        }
    }

    fn invalidate(&self) {
        if self.hwnd.is_null() {
            return;
        }
        unsafe {
            InvalidateRect(self.hwnd, std::ptr::null(), 0);
        }
    }

    fn text_range(&self) -> Option<(usize, usize)> {
        let a = self.text_anchor?;
        let b = self.text_current?;
        Some((a.min(b), a.max(b)))
    }

    fn selected_text(&self) -> String {
        let Some((a, b)) = self.text_range() else {
            return String::new();
        };
        ocr::join_words(&self.words, a, b)
    }

    fn full_text(&self) -> String {
        if self.words.is_empty() {
            String::new()
        } else {
            ocr::join_words(&self.words, 0, self.words.len() - 1)
        }
    }

    fn hit_button(&self, x: i32, y: i32) -> Option<usize> {
        self.buttons.iter().rposition(|hit| {
            x >= hit.rect.left && x < hit.rect.right && y >= hit.rect.top && y < hit.rect.bottom
        })
    }

    fn monitor_at(&self, x: i32, y: i32) -> Option<PxRect> {
        self.monitors
            .iter()
            .copied()
            .find(|mon| mon.contains(x, y))
            .or(Some(PxRect {
                x: 0,
                y: 0,
                w: self.shot_w,
                h: self.shot_h,
            }))
    }

    fn start_ocr(&mut self) {
        let Some(sel) = self.clamped() else {
            self.status = "Trace un rectangle".into();
            self.pending_web = false;
            self.invalidate();
            return;
        };
        if sel.w < 12 || sel.h < 12 {
            self.status = "Trace un rectangle".into();
            self.pending_web = false;
            self.invalidate();
            return;
        }
        let Some(crop) = pixels::crop(&self.bgra, self.shot_w, sel) else {
            self.pending_web = false;
            return;
        };
        self.ocr_token = self.ocr_token.wrapping_add(1);
        let token = self.ocr_token;
        let (tx, rx) = mpsc::channel();
        self.ocr_rx = Some(rx);
        let w = sel.w;
        let h = sel.h;
        let ox = sel.x as f32;
        let oy = sel.y as f32;
        let hwnd = self.hwnd as usize;
        std::thread::Builder::new()
            .name("lens-ocr-overlay".into())
            .spawn(move || {
                let result = ocr::recognize(&crop, w, h).map(|mut words| {
                    for word in &mut words {
                        word.x += ox;
                        word.y += oy;
                    }
                    words
                });
                let _ = tx.send(OcrDone { token, result });
                unsafe {
                    PostMessageW(hwnd as HWND, WM_OCR, 0, 0);
                }
            })
            .ok();
        self.status = "Lecture…".into();
        self.invalidate();
    }

    fn poll_ocr(&mut self) {
        let Some(rx) = &self.ocr_rx else {
            return;
        };
        let Ok(done) = rx.try_recv() else {
            return;
        };
        self.ocr_rx = None;
        if done.token != self.ocr_token {
            return;
        }
        let web = self.pending_web;
        self.pending_web = false;
        match done.result {
            Ok(words) => {
                let empty = words.is_empty();
                let preview = if empty {
                    String::new()
                } else {
                    ocr::join_words(&words, 0, words.len() - 1)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                self.words = words;
                self.text_anchor = None;
                self.text_current = None;
                if web && !empty {
                    self.commit(CloseAction::Web);
                    return;
                }
                self.status = if empty {
                    "Aucun texte".into()
                } else if self.words.len() == 1 {
                    format!("1 mot · {preview}")
                } else {
                    let shown: String = preview.chars().take(48).collect();
                    format!("{} mots · {shown}", self.words.len())
                };
            }
            Err(err) => {
                log_line(&format!("ocr {err}"));
                self.status = err;
                self.words.clear();
            }
        }
        self.invalidate();
    }

    fn commit(&mut self, action: CloseAction) {
        if self.clamped().is_none()
            && let Some((x, y)) = Some(self.cursor())
            && let Some(mon) = self.monitor_at(x, y)
        {
            self.sel = Some(mon);
            self.words.clear();
        }
        let Some(sel) = self.clamped() else {
            self.status = "Trace un rectangle".into();
            self.invalidate();
            return;
        };
        let selection = self.selected_text();
        if action == CloseAction::Web && selection.is_empty() && self.words.is_empty() {
            if !self.pending_web {
                self.pending_web = true;
                self.ocr_please = true;
            } else {
                self.pending_web = false;
                self.status = "Pas de texte".into();
                self.invalidate();
            }
            return;
        }
        self.finish_pen(Some(self.cursor()));
        let Some(bgra) = self.baked(sel) else {
            return;
        };
        self.outcome = Some(Grab {
            bgra,
            width: sel.w,
            height: sel.h,
            action,
            text: self.full_text(),
            selection,
            screen_x: sel.x + self.origin_x,
            screen_y: sel.y + self.origin_y,
        });
        self.done = true;
    }

    fn copy_color(&mut self) {
        let (x, y) = self.cursor();
        let Some(hex) = pixels::sample_hex(&self.bgra, self.shot_w, self.shot_h, x, y) else {
            return;
        };
        self.status = match clipboard::copy_text(&hex) {
            Ok(()) => format!("{hex} copié"),
            Err(err) => err,
        };
        self.invalidate();
    }

    fn on_escape(&mut self) {
        if self.pen.take().is_some() {
            self.invalidate();
            return;
        }
        if self.drawing {
            self.drawing = false;
            self.invalidate();
            return;
        }
        if self.text_anchor.is_some() {
            self.text_anchor = None;
            self.text_current = None;
            self.invalidate();
            return;
        }
        if self.sel.is_some() {
            self.sel = None;
            self.words.clear();
            self.buttons.clear();
            self.status.clear();
            self.invalidate();
            return;
        }
        self.done = true;
    }

    fn nudge(&mut self, dx: i32, dy: i32) {
        let Some(sel) = self.sel else {
            return;
        };
        let step = if unsafe { GetKeyState(VK_SHIFT as i32) } < 0 {
            10
        } else {
            1
        };
        self.sel = Some(PxRect {
            x: sel.x + dx * step,
            y: sel.y + dy * step,
            ..sel
        });
        self.words.clear();
        self.text_anchor = None;
        self.text_current = None;
        self.invalidate();
    }

    fn on_char(&mut self, ch: char) {
        match ch {
            'c' | 'C' => self.commit(CloseAction::Copy),
            't' | 'T' => self.ocr_please = true,
            'g' | 'G' => self.commit(CloseAction::Lens),
            's' | 'S' => self.commit(CloseAction::Save),
            'p' | 'P' => self.commit(CloseAction::Pin),
            'w' | 'W' => self.commit(CloseAction::Web),
            'k' | 'K' => self.copy_color(),
            'd' | 'D' => self.toggle_draw(),
            _ => {}
        }
    }

    fn toggle_draw(&mut self) {
        self.finish_pen(Some(self.cursor()));
        self.drawing = !self.drawing;
        self.invalidate();
    }

    fn begin_pen(&mut self, x: i32, y: i32) {
        let scale = self.scale_at(x, y);
        let preset = PEN_WIDTHS.get(self.pen_width).copied().unwrap_or(4.0);
        let width = (preset * scale).round().clamp(1.0, 28.0) as i32;
        let color = PEN_COLORS
            .get(self.pen_color)
            .copied()
            .unwrap_or(PEN_COLORS[0]);
        self.pen = Some(Stroke {
            pts: vec![(x, y)],
            width,
            color,
        });
        self.anchor = None;
        self.dragged = false;
        self.text_drag = false;
        self.button_arm = None;
    }

    fn extend_pen(&mut self, x: i32, y: i32) {
        let Some(pen) = &mut self.pen else {
            return;
        };
        let Some((lx, ly)) = pen.pts.last().copied() else {
            pen.pts.push((x, y));
            return;
        };
        let dx = x - lx;
        let dy = y - ly;
        if dx * dx + dy * dy >= 4 {
            pen.pts.push((x, y));
        }
    }

    fn finish_pen(&mut self, at: Option<(i32, i32)>) {
        let Some(mut stroke) = self.pen.take() else {
            return;
        };
        if let Some((x, y)) = at
            && stroke.pts.last().copied() != Some((x, y))
        {
            stroke.pts.push((x, y));
        }
        if !stroke.pts.is_empty() {
            self.strokes.push(stroke);
        }
    }

    fn hit_draw(&self, x: i32, y: i32) -> bool {
        self.draw_chips
            .iter()
            .any(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
    }

    fn pen_kind_at(&self, x: i32, y: i32) -> Option<PenKind> {
        self.pen_hits
            .iter()
            .find(|hit| {
                x >= hit.rect.left && x < hit.rect.right && y >= hit.rect.top && y < hit.rect.bottom
            })
            .map(|hit| hit.kind)
    }

    fn apply_pen(&mut self, kind: PenKind) {
        match kind {
            PenKind::Color(index) if index < PEN_COLORS.len() => self.pen_color = index,
            PenKind::Width(index) if index < PEN_WIDTHS.len() => self.pen_width = index,
            _ => {}
        }
    }

    fn baked(&self, sel: PxRect) -> Option<Vec<u8>> {
        let mut bgra = pixels::crop(&self.bgra, self.shot_w, sel)?;
        let has_ink = self.strokes.iter().any(|stroke| !stroke.pts.is_empty())
            || self
                .pen
                .as_ref()
                .is_some_and(|stroke| !stroke.pts.is_empty());
        if !has_ink {
            return Some(bgra);
        }
        let screen = unsafe { GetDC(std::ptr::null_mut()) };
        if screen.is_null() {
            return Some(bgra);
        }
        let dib = Dib::new(screen, &bgra, sel.w, sel.h);
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
        let Ok(dib) = dib else {
            return Some(bgra);
        };
        self.paint_ink(dib.dc, -sel.x, -sel.y);
        unsafe {
            GdiFlush();
            let len = (sel.w as usize)
                .saturating_mul(sel.h as usize)
                .saturating_mul(4);
            if !dib.bits.is_null() && len == bgra.len() {
                std::ptr::copy_nonoverlapping(dib.bits, bgra.as_mut_ptr(), len);
            }
        }
        // GDI leaves the alpha byte at zero on the pixels it paints.
        pixels::force_opaque(&mut bgra);
        Some(bgra)
    }

    fn on_key(&mut self, vk: u16) {
        let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        if ctrl && vk == b'Z' as u16 {
            let changed = if self.pen.take().is_some() {
                true
            } else {
                self.strokes.pop().is_some()
            };
            if changed {
                self.invalidate();
            }
            return;
        }
        if ctrl && vk == b'C' as u16 {
            let text = self.selected_text();
            if text.is_empty() {
                self.commit(CloseAction::Copy);
            } else {
                self.status = match clipboard::copy_text(&text) {
                    Ok(()) => "Texte copié".into(),
                    Err(err) => err,
                };
                self.invalidate();
            }
            return;
        }
        if ctrl && vk == b'A' as u16 && !self.words.is_empty() {
            self.text_anchor = Some(0);
            self.text_current = Some(self.words.len() - 1);
            self.invalidate();
            return;
        }
        match vk {
            VK_ESCAPE => self.on_escape(),
            VK_RETURN => self.commit(CloseAction::Copy),
            VK_LEFT => self.nudge(-1, 0),
            VK_RIGHT => self.nudge(1, 0),
            VK_UP => self.nudge(0, -1),
            VK_DOWN => self.nudge(0, 1),
            _ => {}
        }
    }

    fn layout(&mut self, hdc: HDC, sel: PxRect) {
        let scale = self.scale_at(sel.x + sel.w / 2, sel.y + sel.h / 2);
        let font = self.font_for(hdc, scale);
        let mut items = vec![
            ("Copier", HitCmd::Close(CloseAction::Copy), true),
            ("Texte", HitCmd::Ocr, false),
            ("Lens", HitCmd::Close(CloseAction::Lens), false),
            ("Sauver", HitCmd::Close(CloseAction::Save), false),
            ("Épingler", HitCmd::Close(CloseAction::Pin), false),
        ];
        if !self.words.is_empty() || !self.selected_text().is_empty() {
            items.push(("Web", HitCmd::Close(CloseAction::Web), false));
        }
        let gap = (6.0 * scale) as i32;
        let height = (32.0 * scale) as i32;
        let pad = (14.0 * scale) as i32;
        let widths: Vec<i32> = items
            .iter()
            .map(|(label, _, _)| text_width(hdc, font, label) + pad * 2)
            .collect();
        let total = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1).max(0);
        let x0 = (sel.x + sel.w / 2 - total / 2).clamp(8, (self.shot_w - total - 8).max(8));
        let below = sel.y + sel.h + (12.0 * scale) as i32;
        let above = sel.y - height - (12.0 * scale) as i32;
        let y = if below + height < self.shot_h - 8 {
            below
        } else if above > 8 {
            above
        } else {
            (sel.y + 8).min((self.shot_h - height - 8).max(0))
        };
        let mut x = x0;
        self.buttons.clear();
        for ((label, cmd, primary), width) in items.into_iter().zip(widths) {
            self.buttons.push(Hit {
                rect: RECT {
                    left: x,
                    top: y,
                    right: x + width,
                    bottom: y + height,
                },
                cmd,
                label,
                primary,
            });
            x += width + gap;
        }
    }

    fn paint(&mut self) {
        unsafe {
            let mut paint = windows_sys::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            let window = BeginPaint(self.hwnd, &mut paint);
            if window.is_null() {
                return;
            }
            let hdc = self.back.dc;
            BitBlt(
                hdc,
                0,
                0,
                self.shot_w,
                self.shot_h,
                self.veil.dc,
                0,
                0,
                SRCCOPY,
            );
            let sel = self.clamped();
            if let Some(sel) = sel {
                BitBlt(
                    hdc,
                    sel.x,
                    sel.y,
                    sel.w,
                    sel.h,
                    self.src.dc,
                    sel.x,
                    sel.y,
                    SRCCOPY,
                );
            }
            GdiFlush();
            self.paint_reveal();
            GdiFlush();
            self.paint_ink(hdc, 0, 0);
            if let Some(sel) = sel {
                self.paint_words(hdc);
                self.paint_frame(hdc, sel);
                self.layout(hdc, sel);
                self.paint_bar(hdc);
                self.paint_size(hdc, sel);
            } else {
                self.buttons.clear();
            }
            self.paint_monitors(hdc);
            self.paint_corner(hdc);
            self.paint_cursor(hdc);
            self.paint_status(hdc);
            self.paint_pen_box(hdc);
            BitBlt(
                window,
                0,
                0,
                self.shot_w,
                self.shot_h,
                self.back.dc,
                0,
                0,
                SRCCOPY,
            );
            EndPaint(self.hwnd, &paint);
        }
    }

    fn paint_reveal(&self) {
        let (x, y) = self.cursor();
        let w = self.src.w;
        let h = self.src.h;
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let scale = self.scale_at(x, y);
        let radius = (128.0 * scale).round() as i32;
        let len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
        unsafe {
            let back = std::slice::from_raw_parts_mut(self.back.bits, len);
            let src = std::slice::from_raw_parts(self.src.bits, len);
            pixels::reveal(back, src, w, h, x, y, radius);
        }
    }

    fn over_button(&self, x: i32, y: i32) -> bool {
        self.hit_button(x, y).is_some() || self.hit_draw(x, y) || self.pen_kind_at(x, y).is_some()
    }

    fn paint_words(&self, hdc: HDC) {
        let Some((start, end)) = self.text_range() else {
            self.paint_underlines(hdc, None);
            return;
        };
        self.paint_underlines(hdc, Some((start, end)));
    }

    fn paint_underlines(&self, hdc: HDC, range: Option<(usize, usize)>) {
        for (index, word) in self.words.iter().enumerate() {
            let selected = range.is_some_and(|(a, b)| index >= a && index <= b);
            let rect = RECT {
                left: word.x.floor() as i32,
                top: word.y.floor() as i32,
                right: (word.x + word.w).ceil() as i32,
                bottom: (word.y + word.h).ceil() as i32,
            };
            if selected {
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 110,
                    AlphaFormat: 0,
                };
                unsafe {
                    AlphaBlend(
                        hdc,
                        rect.left,
                        rect.top,
                        (rect.right - rect.left).max(1),
                        (rect.bottom - rect.top).max(1),
                        self.stamp.dc,
                        0,
                        0,
                        1,
                        1,
                        blend,
                    );
                }
            }
            let line = RECT {
                top: rect.bottom.saturating_sub(1),
                bottom: rect.bottom,
                ..rect
            };
            unsafe {
                FillRect(hdc, &line, if selected { self.white } else { self.gray });
            }
        }
    }

    fn paint_frame(&self, hdc: HDC, sel: PxRect) {
        let rect = RECT {
            left: sel.x,
            top: sel.y,
            right: sel.x + sel.w,
            bottom: sel.y + sel.h,
        };
        unsafe {
            FrameRect(hdc, &inflate(rect, 2), self.black);
            FrameRect(hdc, &inflate(rect, 1), self.white);
        }
    }

    fn paint_bar(&mut self, hdc: HDC) {
        let (x, y) = self.cursor();
        let scale = self
            .buttons
            .first()
            .map(|hit| self.scale_at(hit.rect.left, hit.rect.top))
            .unwrap_or(1.0);
        let font = self.font_for(hdc, scale);
        for hit in &self.buttons {
            let hot = x >= hit.rect.left
                && x < hit.rect.right
                && y >= hit.rect.top
                && y < hit.rect.bottom;
            let fill = if hit.primary {
                self.white
            } else if hot {
                self.hover
            } else {
                self.black
            };
            unsafe {
                FillRect(hdc, &hit.rect, fill);
                FrameRect(
                    hdc,
                    &hit.rect,
                    if hit.primary { self.white } else { self.stroke },
                );
                let color: COLORREF = if hit.primary {
                    gdi::bgr(0, 0, 0)
                } else {
                    gdi::bgr(244, 244, 242)
                };
                text_in(
                    hdc,
                    font,
                    hit.rect,
                    hit.label,
                    color,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }
    }

    fn paint_size(&mut self, hdc: HDC, sel: PxRect) {
        let scale = self.scale_at(sel.x, sel.y);
        let font = self.font_for(hdc, scale);
        let label = format!("{} × {}", sel.w, sel.h);
        let width = text_width(hdc, font, &label) + (16.0 * scale) as i32;
        let height = (22.0 * scale) as i32;
        let mut top = sel.y - height - (8.0 * scale) as i32;
        if let Some(bar) = self.buttons.first()
            && (bar.rect.top - sel.y).abs() < height + 20
            && bar.rect.top < sel.y
        {
            top = bar.rect.top - height - 4;
        }
        if top < 4 {
            top = sel.y + (8.0 * scale) as i32;
        }
        let rect = RECT {
            left: sel.x,
            top,
            right: sel.x + width,
            bottom: top + height,
        };
        unsafe {
            FillRect(hdc, &rect, self.black);
            FrameRect(hdc, &rect, self.stroke);
            text_in(
                hdc,
                font,
                rect,
                &label,
                gdi::bgr(244, 244, 242),
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    fn paint_monitors(&self, hdc: HDC) {
        for mon in &self.monitors {
            let rect = RECT {
                left: mon.x,
                top: mon.y,
                right: mon.x + mon.w,
                bottom: mon.y + mon.h,
            };
            unsafe {
                FrameRect(hdc, &rect, self.stroke);
            }
        }
    }

    fn paint_cursor(&mut self, hdc: HDC) {
        if self.drawing {
            return;
        }
        let (x, y) = self.cursor();
        if x < 0 || y < 0 || x >= self.shot_w || y >= self.shot_h {
            return;
        }
        let scale = self.scale_at(x, y);
        self.paint_ring(hdc, x, y, scale);
        if !self.over_button(x, y) {
            self.paint_loupe(hdc, x, y, scale);
        }
    }

    fn paint_ring(&self, hdc: HDC, x: i32, y: i32, scale: f32) {
        let r = (22.0 * scale).round().clamp(16.0, 48.0) as i32;
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, gdi::bgr(186, 206, 220));
            let old_pen = SelectObject(hdc, pen);
            let old_brush = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));
            Ellipse(hdc, x - r, y - r, x + r + 1, y + r + 1);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(pen);
        }
    }

    fn paint_loupe(&self, hdc: HDC, x: i32, y: i32, scale: f32) {
        let cell = (7.0 * scale).round().clamp(5.0, 12.0) as i32;
        let cells = 11.min(self.shot_w).min(self.shot_h).max(1);
        let grid = cell * cells;
        let bar = (11.0 * scale).round().clamp(8.0, 18.0) as i32;
        let ring = (22.0 * scale).round().clamp(16.0, 48.0) as i32;
        let gap = ring + (8.0 * scale).round() as i32;
        let width = grid + 2;
        let height = grid + bar + 3;
        let mon = self
            .monitors
            .iter()
            .copied()
            .find(|mon| mon.contains(x, y))
            .unwrap_or(PxRect {
                x: 0,
                y: 0,
                w: self.shot_w,
                h: self.shot_h,
            });
        let mut left = x - width / 2;
        let mut top = y - gap - height;
        if top < mon.y + 4 {
            top = y + gap;
        }
        left = left.clamp(mon.x + 4, (mon.x + mon.w - width - 4).max(mon.x + 4));
        top = top.clamp(mon.y + 4, (mon.y + mon.h - height - 4).max(mon.y + 4));
        let half = cells / 2;
        let sx = (x - half).clamp(0, (self.shot_w - cells).max(0));
        let sy = (y - half).clamp(0, (self.shot_h - cells).max(0));
        let mark_x = (x - sx).clamp(0, cells - 1);
        let mark_y = (y - sy).clamp(0, cells - 1);
        unsafe {
            let outer = RECT {
                left,
                top,
                right: left + width,
                bottom: top + height,
            };
            FillRect(hdc, &outer, self.black);
            SetStretchBltMode(hdc, HALFTONE);
            SetBrushOrgEx(hdc, 0, 0, std::ptr::null_mut());
            StretchBlt(
                hdc,
                left + 1,
                top + 1,
                grid,
                grid,
                self.src.dc,
                sx,
                sy,
                cells,
                cells,
                SRCCOPY,
            );
            let mark = RECT {
                left: left + 1 + mark_x * cell,
                top: top + 1 + mark_y * cell,
                right: left + 1 + (mark_x + 1) * cell,
                bottom: top + 1 + (mark_y + 1) * cell,
            };
            FrameRect(hdc, &mark, self.white);
            let color = self.sample_color(x, y);
            let swatch = CreateSolidBrush(color);
            let bar_rect = RECT {
                left: left + 1,
                top: top + 1 + grid + 1,
                right: left + 1 + grid,
                bottom: top + height - 1,
            };
            FillRect(hdc, &bar_rect, swatch);
            DeleteObject(swatch);
            FrameRect(hdc, &outer, self.pale);
        }
    }

    fn sample_color(&self, x: i32, y: i32) -> COLORREF {
        if x < 0 || y < 0 || x >= self.shot_w || y >= self.shot_h {
            return gdi::bgr(0, 0, 0);
        }
        let i = ((y * self.shot_w + x) * 4) as usize;
        if i + 2 >= self.bgra.len() {
            return gdi::bgr(0, 0, 0);
        }
        gdi::bgr(self.bgra[i + 2], self.bgra[i + 1], self.bgra[i])
    }

    fn paint_ink(&self, hdc: HDC, ox: i32, oy: i32) {
        for stroke in &self.strokes {
            paint_stroke(hdc, stroke, ox, oy);
        }
        if let Some(stroke) = &self.pen {
            paint_stroke(hdc, stroke, ox, oy);
        }
    }

    fn paint_corner(&mut self, hdc: HDC) {
        self.draw_chips.clear();
        let monitors = self.monitors.clone();
        let (cx, cy) = self.cursor();
        for mon in monitors {
            let scale = self.scale_at(mon.x + mon.w / 2, mon.y + mon.h / 2);
            let font = self.font_for(hdc, scale);
            let gap = (8.0 * scale) as i32;
            let margin = (16.0 * scale) as i32;
            let height = (26.0 * scale) as i32;
            let pad = (18.0 * scale) as i32;
            let draw_w = text_width(hdc, font, "D dessin") + pad;
            let pick_w = text_width(hdc, font, "K pipette") + pad;
            let total = draw_w + gap + pick_w;
            let left = mon.x + mon.w - total - margin;
            let top = mon.y + mon.h - height - margin;
            if left < mon.x || top < mon.y {
                continue;
            }
            let draw = RECT {
                left,
                top,
                right: left + draw_w,
                bottom: top + height,
            };
            let pick = RECT {
                left: draw.right + gap,
                top,
                right: draw.right + gap + pick_w,
                bottom: top + height,
            };
            let hot = cx >= draw.left && cx < draw.right && cy >= draw.top && cy < draw.bottom;
            let on = self.drawing;
            self.draw_chips.push(draw);
            self.paint_chip(hdc, font, draw, "D dessin", on, hot);
            self.paint_chip(hdc, font, pick, "K pipette", false, false);
        }
    }

    fn paint_chip(&self, hdc: HDC, font: HFONT, rect: RECT, label: &str, on: bool, hot: bool) {
        let fill = if on {
            self.pale
        } else if hot {
            self.hover
        } else {
            self.black
        };
        let frame = if on || !hot { self.pale } else { self.white };
        let color = if on {
            gdi::bgr(0, 0, 0)
        } else if hot {
            gdi::bgr(244, 244, 242)
        } else {
            gdi::bgr(186, 206, 220)
        };
        unsafe {
            FillRect(hdc, &rect, fill);
            FrameRect(hdc, &rect, frame);
            text_in(
                hdc,
                font,
                rect,
                label,
                color,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    fn paint_pen_box(&mut self, hdc: HDC) {
        self.pen_hits.clear();
        if !self.drawing {
            return;
        }
        let (cx, cy) = self.cursor();
        let Some(mon) = self.monitor_at(cx, cy) else {
            return;
        };
        let scale = self.scale_at(mon.x + mon.w / 2, mon.y + mon.h / 2);
        let pad = (8.0 * scale).round().max(6.0) as i32;
        let swatch = (16.0 * scale).round().max(12.0) as i32;
        let gap = (6.0 * scale).round().max(4.0) as i32;
        let sep = (14.0 * scale).round().max(10.0) as i32;
        let thick = (26.0 * scale).round().max(20.0) as i32;
        let margin = (16.0 * scale).round().max(10.0) as i32;
        let colors = PEN_COLORS.len() as i32;
        let widths = PEN_WIDTHS.len() as i32;
        let colors_w = colors * swatch + (colors - 1) * gap;
        let widths_w = widths * thick + (widths - 1) * gap;
        let box_w = pad * 2 + colors_w + sep + widths_w;
        let box_h = pad * 2 + swatch;
        if box_w > mon.w - 8 || box_h > mon.h - 8 {
            return;
        }
        let left = mon.x + (mon.w - box_w) / 2;
        let top = mon.y + margin;
        let outer = RECT {
            left,
            top,
            right: left + box_w,
            bottom: top + box_h,
        };
        unsafe {
            FillRect(hdc, &outer, self.black);
            FrameRect(hdc, &outer, self.pale);
        }
        let y = top + pad;
        let mut x = left + pad;
        for (index, color) in PEN_COLORS.iter().enumerate() {
            let cell = RECT {
                left: x,
                top: y,
                right: x + swatch,
                bottom: y + swatch,
            };
            let hot = cx >= cell.left && cx < cell.right && cy >= cell.top && cy < cell.bottom;
            self.paint_swatch(hdc, cell, *color, index == self.pen_color, hot);
            self.pen_hits.push(PenHit {
                rect: cell,
                kind: PenKind::Color(index),
            });
            x += swatch + gap;
        }
        let line_x = left + pad + colors_w + sep / 2;
        let line = RECT {
            left: line_x,
            top: y + 2,
            right: line_x + 1,
            bottom: y + swatch - 2,
        };
        unsafe {
            FillRect(hdc, &line, self.gray);
        }
        x = left + pad + colors_w + sep;
        for (index, preset) in PEN_WIDTHS.iter().enumerate() {
            let cell = RECT {
                left: x,
                top: y,
                right: x + thick,
                bottom: y + swatch,
            };
            let hot = cx >= cell.left && cx < cell.right && cy >= cell.top && cy < cell.bottom;
            let bar = (*preset * scale)
                .round()
                .clamp(1.0, (swatch - 6).max(1) as f32) as i32;
            self.paint_weight(hdc, cell, bar, index == self.pen_width, hot);
            self.pen_hits.push(PenHit {
                rect: cell,
                kind: PenKind::Width(index),
            });
            x += thick + gap;
        }
    }

    fn paint_swatch(&self, hdc: HDC, cell: RECT, color: Rgb, on: bool, hot: bool) {
        unsafe {
            if on {
                FillRect(hdc, &cell, self.pale);
                fill_color(hdc, inflate(cell, -2), color);
            } else {
                fill_color(hdc, cell, color);
                FrameRect(hdc, &cell, if hot { self.white } else { self.gray });
            }
        }
    }

    fn paint_weight(&self, hdc: HDC, cell: RECT, bar: i32, on: bool, hot: bool) {
        unsafe {
            if on {
                FillRect(hdc, &cell, self.pale);
            } else {
                FrameRect(hdc, &cell, if hot { self.white } else { self.pale });
            }
        }
        let span = cell.right - cell.left;
        let inset = (span / 5).clamp(3, 8);
        let bar_w = (span - inset * 2).max(4);
        let bar_h = bar.clamp(1, (cell.bottom - cell.top - 4).max(1));
        let cx = cell.left + span / 2;
        let cy = (cell.top + cell.bottom) / 2;
        let mark = RECT {
            left: cx - bar_w / 2,
            top: cy - bar_h / 2,
            right: cx - bar_w / 2 + bar_w,
            bottom: cy - bar_h / 2 + bar_h,
        };
        unsafe {
            FillRect(hdc, &mark, if on { self.black } else { self.white });
        }
    }

    fn paint_status(&mut self, hdc: HDC) {
        if self.status.is_empty() {
            return;
        }
        let (x, y) = self.cursor();
        let Some(mon) = self.monitors.iter().copied().find(|mon| mon.contains(x, y)) else {
            return;
        };
        let scale = self.scale_at(x, y);
        let font = self.font_for(hdc, scale);
        let width = (text_width(hdc, font, &self.status) + (28.0 * scale) as i32)
            .min(mon.w - 24)
            .max(80);
        let height = (28.0 * scale) as i32;
        let left = mon.x + (mon.w - width) / 2;
        let top = mon.y + mon.h - height - (18.0 * scale) as i32;
        let rect = RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
        let line = self.status.clone();
        unsafe {
            FillRect(hdc, &rect, self.black);
            FrameRect(hdc, &rect, self.stroke);
            text_in(
                hdc,
                font,
                rect,
                &line,
                gdi::bgr(244, 244, 242),
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if !self.font.is_null() {
                DeleteObject(self.font);
                self.font = std::ptr::null_mut();
            }
            for brush in [
                self.black,
                self.white,
                self.stroke,
                self.gray,
                self.hover,
                self.pale,
            ] {
                if !brush.is_null() {
                    DeleteObject(brush);
                }
            }
        }
    }
}

fn paint_stroke(hdc: HDC, stroke: &Stroke, ox: i32, oy: i32) {
    if stroke.pts.is_empty() {
        return;
    }
    let ink = stroke.width.max(1);
    let halo = ink_halo(stroke.color);
    stroke_path(
        hdc,
        &stroke.pts,
        ox,
        oy,
        ink + 2,
        gdi::bgr(halo.r, halo.g, halo.b),
    );
    stroke_path(
        hdc,
        &stroke.pts,
        ox,
        oy,
        ink,
        gdi::bgr(stroke.color.r, stroke.color.g, stroke.color.b),
    );
}

fn ink_halo(color: Rgb) -> Rgb {
    let y = color.r as u16 * 3 + color.g as u16 * 6 + color.b as u16;
    if y < 1400 {
        Rgb {
            r: 244,
            g: 244,
            b: 242,
        }
    } else {
        Rgb { r: 0, g: 0, b: 0 }
    }
}

fn fill_color(hdc: HDC, rect: RECT, color: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(gdi::bgr(color.r, color.g, color.b));
        FillRect(hdc, &rect, brush);
        DeleteObject(brush);
    }
}

fn stroke_path(hdc: HDC, pts: &[(i32, i32)], ox: i32, oy: i32, width: i32, color: COLORREF) {
    if pts.is_empty() || width < 1 {
        return;
    }
    unsafe {
        if pts.len() == 1 {
            let brush = CreateSolidBrush(color);
            let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
            let old_brush = SelectObject(hdc, brush);
            let (x, y) = pts[0];
            let left = x + ox - width / 2;
            let top = y + oy - width / 2;
            Ellipse(hdc, left, top, left + width, top + width);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(brush);
            return;
        }
        let brush = LOGBRUSH {
            lbStyle: BS_SOLID,
            lbColor: color,
            lbHatch: 0,
        };
        let pen = ExtCreatePen(
            (PS_GEOMETRIC | PS_ENDCAP_ROUND | PS_JOIN_ROUND | PS_SOLID) as u32,
            width as u32,
            &brush,
            0,
            std::ptr::null(),
        );
        if pen.is_null() {
            return;
        }
        let old = SelectObject(hdc, pen);
        let points: Vec<POINT> = pts
            .iter()
            .map(|(x, y)| POINT {
                x: x + ox,
                y: y + oy,
            })
            .collect();
        Polyline(hdc, points.as_ptr(), points.len() as i32);
        SelectObject(hdc, old);
        DeleteObject(pen);
    }
}

fn brush(r: u8, g: u8, b: u8) -> HBRUSH {
    unsafe { CreateSolidBrush(gdi::bgr(r, g, b)) }
}

fn inflate(rect: RECT, px: i32) -> RECT {
    RECT {
        left: rect.left - px,
        top: rect.top - px,
        right: rect.right + px,
        bottom: rect.bottom + px,
    }
}

fn text_width(hdc: HDC, font: HFONT, text: &str) -> i32 {
    unsafe {
        windows_sys::Win32::Graphics::Gdi::SelectObject(hdc, font);
        let units = wide(text);
        let mut size = SIZE::default();
        GetTextExtentPoint32W(hdc, units.as_ptr(), (units.len() - 1) as i32, &mut size);
        size.cx.max(8)
    }
}

fn text_in(hdc: HDC, font: HFONT, mut rect: RECT, text: &str, color: COLORREF, format: u32) {
    unsafe {
        windows_sys::Win32::Graphics::Gdi::SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, color);
        let units = wide(text);
        DrawTextW(hdc, units.as_ptr(), -1, &mut rect, format);
    }
}

fn focus(hwnd: HWND) {
    unsafe {
        let fore = GetForegroundWindow();
        let fore_thread = GetWindowThreadProcessId(fore, std::ptr::null_mut());
        let me = GetCurrentThreadId();
        if fore_thread != 0 && fore_thread != me {
            AttachThreadInput(fore_thread, me, 1);
        }
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        if fore_thread != 0 && fore_thread != me {
            AttachThreadInput(fore_thread, me, 0);
        }
    }
}

fn class_name() -> *const u16 {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| wide("LensOverlay")).as_ptr()
}

fn register_class() {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        let mut klass = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW
                | CS_VREDRAW
                | windows_sys::Win32::UI::WindowsAndMessaging::CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_CROSS) },
            lpszClassName: class_name(),
            ..WNDCLASSEXW::default()
        };
        klass.hbrBackground = std::ptr::null_mut();
        let atom = unsafe { RegisterClassExW(&klass) };
        if atom == 0 {
            log_line("classe LensOverlay");
        }
    });
}

fn state_from(hwnd: HWND) -> Option<*mut Overlay> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Overlay;
    if raw.is_null() { None } else { Some(raw) }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let created = &*(lparam as *const CREATESTRUCTW);
            let state = created.lpCreateParams as *mut Overlay;
            if !state.is_null() {
                (*state).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_ERASEBKGND {
            return 1;
        }
        let Some(state) = state_from(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        let overlay = &mut *state;
        match msg {
            WM_PAINT => {
                overlay.paint();
                0
            }
            WM_SETCURSOR => {
                let (x, y) = overlay.cursor();
                let id = if overlay.hit_button(x, y).is_some()
                    || overlay.hit_draw(x, y)
                    || overlay.pen_kind_at(x, y).is_some()
                {
                    IDC_ARROW
                } else if !overlay.drawing
                    && !overlay.words.is_empty()
                    && overlay.sel.is_some_and(|sel| sel.contains(x, y))
                    && ocr::hit_word(&overlay.words, x as f32, y as f32).is_some()
                {
                    IDC_IBEAM
                } else {
                    IDC_CROSS
                };
                SetCursor(LoadCursorW(std::ptr::null_mut(), id));
                1
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                let (x, y) = overlay.cursor();
                if let Some(kind) = overlay.pen_kind_at(x, y) {
                    overlay.pen_arm = Some(kind);
                    overlay.anchor = None;
                    overlay.button_arm = None;
                    return 0;
                }
                if overlay.hit_draw(x, y) {
                    overlay.draw_arm = true;
                    overlay.anchor = None;
                    overlay.button_arm = None;
                    return 0;
                }
                if let Some(index) = overlay.hit_button(x, y) {
                    overlay.button_arm = Some(index);
                    overlay.anchor = None;
                    return 0;
                }
                if overlay.drawing {
                    overlay.begin_pen(x, y);
                    overlay.invalidate();
                    return 0;
                }
                if !overlay.words.is_empty()
                    && overlay.sel.is_some_and(|sel| sel.contains(x, y))
                    && let Some(word) = ocr::hit_word(&overlay.words, x as f32, y as f32)
                {
                    overlay.text_anchor = Some(word);
                    overlay.text_current = Some(word);
                    overlay.text_drag = true;
                    overlay.anchor = None;
                    overlay.invalidate();
                    return 0;
                }
                overlay.button_arm = None;
                overlay.text_drag = false;
                overlay.anchor = Some((x, y));
                overlay.dragged = false;
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = overlay.cursor();
                if overlay.pen.is_some() {
                    overlay.extend_pen(x, y);
                    overlay.invalidate();
                    return 0;
                }
                if overlay.text_drag
                    && let Some(word) = ocr::hit_word(&overlay.words, x as f32, y as f32)
                {
                    overlay.text_current = Some(word);
                } else if let Some(anchor) = overlay.anchor {
                    let dx = x - anchor.0;
                    let dy = y - anchor.1;
                    if dx.abs() > 3 || dy.abs() > 3 {
                        if !overlay.dragged {
                            overlay.ocr_token = overlay.ocr_token.wrapping_add(1);
                            overlay.ocr_rx = None;
                        }
                        overlay.dragged = true;
                        overlay.words.clear();
                        overlay.text_anchor = None;
                        overlay.text_current = None;
                        let shift = GetKeyState(VK_SHIFT as i32) < 0;
                        overlay.sel = Some(if shift {
                            PxRect::square(anchor, (x, y))
                        } else {
                            PxRect::from_points(anchor, (x, y))
                        });
                    }
                }
                overlay.invalidate();
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let (x, y) = overlay.cursor();
                if let Some(kind) = overlay.pen_arm.take() {
                    if overlay.pen_kind_at(x, y) == Some(kind) {
                        overlay.apply_pen(kind);
                    }
                    overlay.invalidate();
                    return 0;
                }
                if overlay.draw_arm {
                    overlay.draw_arm = false;
                    if overlay.hit_draw(x, y) {
                        overlay.toggle_draw();
                    }
                    return 0;
                }
                if overlay.pen.is_some() {
                    overlay.finish_pen(Some((x, y)));
                    overlay.invalidate();
                    return 0;
                }
                if let Some(index) = overlay.button_arm.take() {
                    if overlay.hit_button(x, y) == Some(index) {
                        match overlay.buttons.get(index).map(|hit| hit.cmd) {
                            Some(HitCmd::Ocr) => overlay.ocr_please = true,
                            Some(HitCmd::Close(action)) => overlay.commit(action),
                            None => {}
                        }
                    }
                    return 0;
                }
                if overlay.text_drag {
                    overlay.text_drag = false;
                    overlay.invalidate();
                    return 0;
                }
                overlay.anchor = None;
                if overlay.dragged {
                    if overlay.clamped().is_none() {
                        overlay.sel = None;
                    } else {
                        overlay.ocr_please = true;
                    }
                    overlay.dragged = false;
                    overlay.invalidate();
                }
                0
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = overlay.cursor();
                if overlay.drawing
                    || overlay.hit_draw(x, y)
                    || overlay.hit_button(x, y).is_some()
                    || overlay.pen_kind_at(x, y).is_some()
                {
                    return 0;
                }
                if overlay.clamped().is_some() {
                    overlay.commit(CloseAction::Copy);
                }
                0
            }
            WM_RBUTTONUP | WM_CLOSE => {
                overlay.done = true;
                0
            }
            WM_KEYDOWN => {
                overlay.on_key(wparam as u16);
                0
            }
            WM_SYSKEYDOWN => {
                if wparam as u16 == VK_F4 || wparam as u16 == VK_ESCAPE {
                    overlay.done = true;
                }
                0
            }
            WM_OCR => {
                overlay.poll_ocr();
                0
            }
            WM_CHAR => {
                if let Some(ch) = char::from_u32(wparam as u32)
                    && ch >= ' '
                {
                    overlay.on_char(ch);
                }
                0
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_paints_on_the_dib() {
        let screen = unsafe { GetDC(std::ptr::null_mut()) };
        assert!(!screen.is_null());
        let bgra = vec![255u8; 32 * 32 * 4];
        let dib = Dib::new(screen, &bgra, 32, 32).expect("dib");
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
        paint_stroke(
            dib.dc,
            &Stroke {
                pts: vec![(2, 16), (29, 16)],
                width: 3,
                color: Rgb {
                    r: 186,
                    g: 206,
                    b: 220,
                },
            },
            0,
            0,
        );
        unsafe { GdiFlush() };
        let px = unsafe { std::slice::from_raw_parts(dib.bits, bgra.len()) };
        let i = (16 * 32 + 16) * 4;
        assert_eq!(&px[i..i + 3], &[186, 206, 220]);
        assert_eq!(&px[0..3], &[255, 255, 255]);
    }
}
