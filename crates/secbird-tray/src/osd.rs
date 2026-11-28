//! The small popup above the clock: a DPI change, a profile switch, the on-board profile.
//! It shows for a moment and fades away. It never takes focus and lets clicks through, and
//! it shows over full-screen windows and borderless games, but not over a game in exclusive
//! full screen or in presentation mode.
//!
//! Plain GDI on a layered window, whose alpha does the fades. It follows Windows' app theme
//! and accent colour, and is drawn in real pixels at the scale of the screen it is on (the
//! process is PerMonitorV2). Made on first use; nothing runs while it is hidden.

use crate::theme;
use std::cell::{Cell, RefCell};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "SecretarybirdOsd";
const TIMER_HIDE: usize = 1;
/// One step of a fade or of the bar moving; only runs while one is under way.
const TIMER_FRAME: usize = 2;
const FRAME_MS: u32 = 15;
const SHOW_FOR: u32 = 1500;
const FADE_IN: Duration = Duration::from_millis(120);
const FADE_OUT: Duration = Duration::from_millis(200);
const BAR_MOVE: Duration = Duration::from_millis(220);

/// What the popup says. Made by [`Popup::dpi`], [`Popup::profile`] and [`Popup::onboard`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Popup {
    /// A small line above the main one, in the accent colour ("Auto · game.exe").
    pub caption: String,
    /// The main line: a DPI, a profile name.
    pub big: String,
    /// Smaller, right after `big` on its baseline ("DPI").
    pub unit: String,
    /// Small, at the right end of the main line ("Stage 2 of 5").
    pub aside: String,
    /// How full the bar is, 0 to 1; no bar when `None`.
    pub bar: Option<f32>,
    /// The last line: which device.
    pub detail: String,
    /// `big` is a number, shown larger than a name.
    pub numeric: bool,
}

/// Why a profile popup shows.
pub enum Why<'a> {
    /// Picked in the tray menu.
    Menu,
    /// The mouse's profile button.
    Button,
    /// This program came to the front, and a profile is tied to it.
    Auto(&'a str),
    /// That program left the front: back to the profile in use before it.
    Back(&'a str),
}

impl Popup {
    /// A DPI change: the DPI, how far it is up the device's range (`max`, when known), the
    /// stage it is when it is one of the device's stages (`(n, of)`), and the device.
    pub fn dpi(x: u16, y: u16, max: Option<u16>, stage: Option<(usize, usize)>, device: &str) -> Popup {
        Popup {
            big: if x == y { x.to_string() } else { format!("{x} × {y}") },
            unit: "DPI".into(),
            aside: stage.map(|(n, of)| format!("Stage {n} of {of}")).unwrap_or_default(),
            bar: max.filter(|&m| m > 0).map(|m| (x.max(y) as f32 / m as f32).clamp(0.0, 1.0)),
            detail: device.into(),
            numeric: true,
            ..Popup::default()
        }
    }

    /// One popup for a profile switch, however many devices it touched: the profile(s), the
    /// device(s), and why.
    pub fn profile(names: &[String], devices: &[String], why: Why) -> Popup {
        let mut unique: Vec<&str> = Vec::new();
        for n in names {
            if !unique.contains(&n.as_str()) {
                unique.push(n);
            }
        }
        let listed = devices.join(", ");
        Popup {
            caption: match why {
                Why::Menu => "Profile".into(),
                Why::Button => "Profile button".into(),
                Why::Auto(exe) => format!("Auto · {exe}"),
                Why::Back(exe) => format!("Auto · back from {exe}"),
            },
            big: unique.join(" · "),
            detail: if devices.len() > 1 && listed.chars().count() > 40 { format!("{} devices", devices.len()) } else { listed },
            ..Popup::default()
        }
    }

    /// The device switched to its on-board profile `slot` (of `count`, when known), with
    /// the DPI that came with it, if it said.
    pub fn onboard(slot: u8, count: u8, device: &str, dpi: Option<(u16, u16)>) -> Popup {
        Popup {
            caption: "On-board".into(),
            big: format!("Profile {slot}"),
            unit: if (1..=count).contains(&slot) { format!("of {count}") } else { String::new() },
            aside: match dpi {
                Some((x, y)) if x == y => format!("{x} DPI"),
                Some((x, y)) => format!("{x} × {y} DPI"),
                None => String::new(),
            },
            detail: device.into(),
            ..Popup::default()
        }
    }
}

/// Show `popup` (replacing what is shown: the bar moves to its new place), then fade it out
/// after a moment. Nothing over a game in exclusive full screen.
pub fn show(popup: Popup) {
    if full_screen_in_front() {
        return;
    }
    unsafe {
        let hwnd = window();
        if hwnd.is_null() {
            return;
        }
        let (work, edge, dpi) = tray_screen();
        let animate = animations_on();
        let colors = colors();
        let ready = STATE.with(|s| {
            let mut guard = s.try_borrow_mut().ok()?;
            let st = guard.as_mut()?;
            if st.fonts.as_ref().is_none_or(|f| f.dpi != dpi) {
                st.fonts = Some(Fonts::new(dpi));
            }
            let layout = Layout::measure(&popup, st.fonts.as_ref()?);
            let now = Instant::now();
            // The bar moves from where it is to the new value when one was already shown.
            let to = popup.bar.unwrap_or(0.0);
            st.bar = if st.visible && animate && st.popup.bar.is_some() && popup.bar.is_some() {
                Bar { from: st.bar.now, to, start: Some(now), now: st.bar.now }
            } else {
                Bar { from: to, to, start: None, now: to }
            };
            // Fade in from where it is: nothing, or partly faded out.
            let fading_out = st.fade.as_ref().is_some_and(|f| f.to == 0.0);
            if !animate {
                st.alpha = 255.0;
                st.fade = None;
            } else if !st.visible {
                st.alpha = 0.0;
                st.fade = Some(Fade { from: 0.0, to: 255.0, start: now, length: FADE_IN });
            } else if fading_out {
                st.fade = Some(Fade { from: st.alpha, to: 255.0, start: now, length: FADE_IN });
            }
            st.popup = popup;
            st.colors = colors;
            st.layout = layout;
            st.visible = true;
            let frames = st.fade.is_some() || st.bar.now != st.bar.to;
            Some((layout.w, layout.h, st.alpha, st.dwm_corners, frames))
        });
        // Busy: only if a popup came in while one was being drawn, which can't happen.
        let Some((w, h, alpha, dwm_corners, frames)) = ready else { return };

        // Windows 11 rounds the corners and draws the border itself; before, a region.
        if dwm_corners {
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &colors.border as *const COLORREF as _, 4);
        } else {
            let r = scaled(dpi, 8.0);
            SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r), 0);
        }
        let margin = scaled(dpi, 12.0);
        let x = if edge.left { work.left + margin } else { work.right - w - margin };
        let y = if edge.top { work.top + margin } else { work.bottom - h - margin };
        SetLayeredWindowAttributes(hwnd, 0, alpha.round() as u8, LWA_ALPHA);
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(hwnd, null(), 0);
        SetTimer(hwnd, TIMER_HIDE, SHOW_FOR, None);
        if frames {
            SetTimer(hwnd, TIMER_FRAME, FRAME_MS, None);
        }
    }
}

/// `v` logical pixels at `dpi`.
fn scaled(dpi: u32, v: f32) -> i32 {
    (v * dpi as f32 / 96.0).round() as i32
}

/// No popup over a game in exclusive full screen (a window on top would make it drop out of
/// that mode, or flicker), nor while Windows is in presentation mode. Borderless full-screen
/// games and full-screen windows (`QUNS_BUSY`) do get it on top: what the user just did with
/// a button, or the profile a game just switched to, should show where they look.
fn full_screen_in_front() -> bool {
    let mut state = 0;
    let ok = unsafe { SHQueryUserNotificationState(&mut state) } == 0;
    ok && matches!(state, QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
}

/// Windows' "Animation effects" (Accessibility > Visual effects). Off: no fades, no moving bar.
fn animations_on() -> bool {
    let mut on: BOOL = TRUE;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut on as *mut BOOL as _, 0) };
    on != 0
}

/// Which corner of the work area the notification area is at.
#[derive(Clone, Copy, Default)]
struct Edge {
    left: bool,
    top: bool,
}

/// The work area of the screen with the notification area (the taskbar with the clock),
/// which corner the clock is in, and that screen's scale.
unsafe fn tray_screen() -> (RECT, Edge, u32) {
    let tray = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
    let monitor = if tray.is_null() {
        MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
    } else {
        MonitorFromWindow(tray, MONITOR_DEFAULTTOPRIMARY)
    };
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..zeroed() };
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut info.rcWork as *mut RECT as _, 0);
        info.rcMonitor = info.rcWork;
    }
    // The taskbar takes its edge out of the work area: top or left when it is there. The
    // clock is at the bottom of a taskbar on the left or right.
    let edge = Edge {
        top: info.rcWork.top > info.rcMonitor.top,
        left: info.rcWork.left > info.rcMonitor.left,
    };
    let (mut dpi, mut unused) = (96, 0);
    if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused) != 0 || dpi == 0 {
        dpi = 96;
    }
    (info.rcWork, edge, dpi)
}

#[derive(Clone, Copy)]
struct Colors {
    background: COLORREF,
    border: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    track: COLORREF,
    accent: COLORREF,
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Secretarybird's own surfaces for Windows' light or dark apps, with Windows' accent for
/// the caption and the bar (Secretarybird's when Windows doesn't say).
fn colors() -> Colors {
    let light = dev_theme().unwrap_or_else(theme::apps_light);
    if light {
        Colors {
            background: rgb(0xFF, 0xFF, 0xFF),
            border: rgb(0xD4, 0xD4, 0xDC),
            text: rgb(0x18, 0x18, 0x1D),
            muted: rgb(0x5F, 0x5F, 0x6B),
            track: rgb(0xE4, 0xE4, 0xEA),
            accent: theme::accent(false).unwrap_or(rgb(0x1D, 0x4E, 0xD8)),
        }
    } else {
        Colors {
            background: rgb(0x1B, 0x1B, 0x1D),
            border: rgb(0x3A, 0x36, 0x32),
            text: rgb(0xF4, 0xEF, 0xE9),
            muted: rgb(0xB8, 0xAE, 0xA3),
            track: rgb(0x34, 0x31, 0x2E),
            accent: theme::accent(true).unwrap_or(rgb(0xFF, 0x7A, 0x45)),
        }
    }
}

/// Development, in a tray that only sees simulated devices (`SECBIRD_SIM_ONLY=1`):
/// `SECBIRD_OSD_THEME=light` or `dark` shows the popup in that theme whatever Windows' is,
/// to look at both without changing Windows' setting.
fn dev_theme() -> Option<bool> {
    if !secbird_core::sim::only() {
        return None;
    }
    match std::env::var("SECBIRD_OSD_THEME").ok()?.as_str() {
        "light" => Some(true),
        "dark" => Some(false),
        _ => None,
    }
}

/// The popup's fonts at one scale. Segoe UI Variable (Windows 11) when it is there.
struct Fonts {
    dpi: u32,
    big_number: Font,
    big_name: Font,
    unit: Font,
    caption: Font,
    small: Font,
}

struct Font {
    handle: HFONT,
    ascent: i32,
    height: i32,
    /// Room for accents above capitals, left out of the line so the line sits tight.
    leading: i32,
}

impl Font {
    unsafe fn new(family: &str, px: f32, weight: u32, dpi: u32) -> Font {
        let handle = CreateFontW(
            -scaled(dpi, px),
            0,
            0,
            0,
            weight as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            wide(family).as_ptr(),
        );
        let dc = CreateCompatibleDC(null_mut());
        let old = SelectObject(dc, handle as _);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(dc, &mut tm);
        SelectObject(dc, old);
        DeleteDC(dc);
        Font { handle, ascent: tm.tmAscent, height: tm.tmHeight, leading: tm.tmInternalLeading }
    }

    /// Width of `text` in this font.
    unsafe fn width(&self, text: &str) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let dc = CreateCompatibleDC(null_mut());
        let old = SelectObject(dc, self.handle as _);
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut size: SIZE = zeroed();
        GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
        SelectObject(dc, old);
        DeleteDC(dc);
        size.cx
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.handle as _) };
    }
}

impl Fonts {
    unsafe fn new(dpi: u32) -> Fonts {
        // GDI sees each weight of Segoe UI Variable as a family of its own (names cut at 31
        // characters); asking the main family for semibold gets the regular one.
        const DISPLAY_SEMIBOLD: &str = "Segoe UI Variable Display Semib";
        const TEXT_SEMIBOLD: &str = "Segoe UI Variable Text Semibold";
        const TEXT: &str = "Segoe UI Variable Text";
        let (display_bold, text_bold, text) = if [DISPLAY_SEMIBOLD, TEXT_SEMIBOLD, TEXT].iter().all(|f| has_font(f)) {
            (DISPLAY_SEMIBOLD, TEXT_SEMIBOLD, TEXT)
        } else {
            ("Segoe UI", "Segoe UI", "Segoe UI")
        };
        Fonts {
            dpi,
            big_number: Font::new(display_bold, 30.0, FW_SEMIBOLD, dpi),
            big_name: Font::new(display_bold, 22.0, FW_SEMIBOLD, dpi),
            unit: Font::new(text_bold, 14.0, FW_SEMIBOLD, dpi),
            caption: Font::new(text_bold, 12.0, FW_SEMIBOLD, dpi),
            small: Font::new(text, 13.0, FW_NORMAL, dpi),
        }
    }

    fn big(&self, numeric: bool) -> &Font {
        if numeric {
            &self.big_number
        } else {
            &self.big_name
        }
    }
}

/// The font family is installed.
unsafe fn has_font(family: &str) -> bool {
    thread_local!(static KNOWN: RefCell<Vec<(String, bool)>> = const { RefCell::new(Vec::new()) });
    if let Some(known) = KNOWN.with(|k| k.borrow().iter().find(|(f, _)| f == family).map(|(_, b)| *b)) {
        return known;
    }
    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, flag: LPARAM) -> i32 {
        *(flag as *mut bool) = true;
        0
    }
    let mut lf: LOGFONTW = zeroed();
    lf.lfCharSet = DEFAULT_CHARSET;
    for (dst, src) in lf.lfFaceName.iter_mut().zip(family.encode_utf16().take(31)) {
        *dst = src;
    }
    let mut yes = false;
    let dc = GetDC(null_mut());
    EnumFontFamiliesExW(dc, &lf, Some(found), &mut yes as *mut bool as LPARAM, 0);
    ReleaseDC(null_mut(), dc);
    KNOWN.with(|k| k.borrow_mut().push((family.to_string(), yes)));
    yes
}

/// Where everything goes, in pixels of the popup's screen.
#[derive(Clone, Copy)]
struct Layout {
    w: i32,
    h: i32,
    caption: RECT,
    /// Where the main line is drawn: it starts above its row by the font's leading.
    big: RECT,
    unit: RECT,
    aside: RECT,
    bar: RECT,
    detail: RECT,
}

impl Default for Layout {
    fn default() -> Self {
        let none = rect(0, 0, 0, 0);
        Layout { w: 0, h: 0, caption: none, big: none, unit: none, aside: none, bar: none, detail: none }
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT { left, top, right, bottom }
}

impl Layout {
    unsafe fn measure(p: &Popup, f: &Fonts) -> Layout {
        let px = |v: f32| scaled(f.dpi, v);
        let (pad_x, pad_top, pad_bottom) = (px(18.0), px(13.0), px(12.0));
        let big = f.big(p.numeric);
        let unit_gap = if p.unit.is_empty() { 0 } else { px(6.0) };
        let aside_gap = if p.aside.is_empty() { 0 } else { px(20.0) };
        let (big_w, unit_w, aside_w) = (big.width(&p.big), f.unit.width(&p.unit), f.small.width(&p.aside));
        let content = [f.caption.width(&p.caption), big_w + unit_gap + unit_w + aside_gap + aside_w, f.small.width(&p.detail)]
            .into_iter()
            .max()
            .unwrap_or(0);
        let w = (content + 2 * pad_x).clamp(px(272.0), px(440.0));
        let inner = w - 2 * pad_x;
        let (left, right) = (pad_x, w - pad_x);

        let mut y = pad_top;
        let mut l = Layout { w, ..Layout::default() };
        if !p.caption.is_empty() {
            l.caption = rect(left, y, right, y + f.caption.height);
            y += f.caption.height + px(3.0);
        }
        // The main line, tight: its leading hangs above the row.
        let row = big.height - big.leading;
        let big_room = (inner - unit_gap - unit_w - aside_gap - aside_w).max(px(40.0));
        let big_right = left + big_w.min(big_room);
        l.big = rect(left, y - big.leading, big_right, y + row);
        let baseline = y - big.leading + big.ascent;
        let unit_top = baseline - f.unit.ascent;
        l.unit = rect(big_right + unit_gap, unit_top, big_right + unit_gap + unit_w, unit_top + f.unit.height);
        let aside_top = baseline - f.small.ascent;
        l.aside = rect(right - aside_w, aside_top, right, aside_top + f.small.height);
        y += row;
        if p.bar.is_some() {
            y += px(9.0);
            l.bar = rect(left, y, right, y + px(4.0));
            y += px(4.0) + px(9.0);
        } else {
            y += px(4.0);
        }
        l.detail = rect(left, y, right, y + f.small.height);
        y += f.small.height;
        l.h = y + pad_bottom;
        l
    }
}

struct Fade {
    from: f32,
    to: f32,
    start: Instant,
    length: Duration,
}

#[derive(Default)]
struct Bar {
    from: f32,
    to: f32,
    start: Option<Instant>,
    now: f32,
}

struct State {
    fonts: Option<Fonts>,
    popup: Popup,
    layout: Layout,
    colors: Colors,
    /// Window alpha now, 0 to 255.
    alpha: f32,
    fade: Option<Fade>,
    bar: Bar,
    /// On screen (maybe fading out).
    visible: bool,
    /// Windows 11 rounds the corners and draws the border.
    dwm_corners: bool,
}

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Smooth stop.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn progress(start: Instant, length: Duration, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0)
}

/// Created on first use: most sessions never show a popup.
unsafe fn window() -> HWND {
    if WINDOW.with(Cell::get).is_null() {
        let hinstance = GetModuleHandleW(null());
        let class = wide(CLASS_NAME);
        let wc = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class.as_ptr(),
            wide("Secretarybird").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        );
        if hwnd.is_null() {
            return hwnd;
        }
        let round = DWMWCP_ROUND;
        let dwm_corners = theme::windows_11()
            && DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &round as *const i32 as _, 4) == 0;
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                fonts: None,
                popup: Popup::default(),
                layout: Layout::default(),
                colors: colors(),
                alpha: 0.0,
                fade: None,
                bar: Bar::default(),
                visible: false,
                dwm_corners,
            })
        });
        WINDOW.with(|c| c.set(hwnd));
    }
    WINDOW.with(Cell::get)
}

/// One step of the fade and of the bar. The frame timer stops once both are done.
unsafe fn frame(hwnd: HWND) {
    let now = Instant::now();
    let step = STATE.with(|s| {
        let mut guard = s.try_borrow_mut().ok()?;
        let st = guard.as_mut()?;
        let mut busy = false;
        if let Some(f) = &st.fade {
            let t = progress(f.start, f.length, now);
            st.alpha = f.from + (f.to - f.from) * ease_out(t);
            if t >= 1.0 {
                st.fade = None;
            } else {
                busy = true;
            }
        }
        let hide = st.visible && st.fade.is_none() && st.alpha < 0.5;
        if hide {
            st.visible = false;
        }
        let moving = st.bar.now != st.bar.to;
        if moving {
            let t = st.bar.start.map_or(1.0, |start| progress(start, BAR_MOVE, now));
            st.bar.now = if t >= 1.0 { st.bar.to } else { st.bar.from + (st.bar.to - st.bar.from) * ease_out(t) };
            busy |= st.bar.now != st.bar.to;
        }
        Some((st.alpha, hide, moving, busy))
    });
    let Some((alpha, hide, moving, busy)) = step else { return };
    SetLayeredWindowAttributes(hwnd, 0, alpha.round() as u8, LWA_ALPHA);
    if hide {
        ShowWindow(hwnd, SW_HIDE);
    }
    if moving {
        InvalidateRect(hwnd, null(), 0);
    }
    if !busy {
        KillTimer(hwnd, TIMER_FRAME);
    }
}

/// Time's up: fade out, or go at once when animations are off.
unsafe fn hide(hwnd: HWND) {
    KillTimer(hwnd, TIMER_HIDE);
    let animate = animations_on();
    let fade = STATE.with(|s| {
        let mut guard = s.try_borrow_mut().ok()?;
        let st = guard.as_mut()?;
        if animate {
            st.fade = Some(Fade { from: st.alpha, to: 0.0, start: Instant::now(), length: FADE_OUT });
        } else {
            st.alpha = 0.0;
            st.fade = None;
            st.visible = false;
        }
        Some(animate)
    });
    match fade {
        Some(true) => {
            SetTimer(hwnd, TIMER_FRAME, FRAME_MS, None);
        }
        Some(false) => {
            ShowWindow(hwnd, SW_HIDE);
        }
        // Busy (can't be: nothing here sends messages while it is borrowed): try again.
        None => {
            SetTimer(hwnd, TIMER_HIDE, FRAME_MS, None);
        }
    }
}

unsafe fn text(dc: HDC, font: &Font, color: COLORREF, s: &str, r: RECT, align: u32) {
    if s.is_empty() {
        return;
    }
    let mut r = r;
    let mut w: Vec<u16> = s.encode_utf16().collect();
    SelectObject(dc, font.handle as _);
    SetTextColor(dc, color);
    DrawTextW(dc, w.as_mut_ptr(), w.len() as i32, &mut r, align | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
}

unsafe fn fill(dc: HDC, r: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(dc, &r, brush);
    DeleteObject(brush as _);
}

/// Draw the popup into `dc` (a memory bitmap the size of the window).
unsafe fn paint(dc: HDC, st: &State) {
    let (l, c, p) = (&st.layout, &st.colors, &st.popup);
    let Some(f) = st.fonts.as_ref() else { return };
    fill(dc, rect(0, 0, l.w, l.h), c.background);
    if !st.dwm_corners {
        // The region cuts the corners; the border follows it.
        let r = scaled(f.dpi, 8.0);
        let region = CreateRoundRectRgn(0, 0, l.w + 1, l.h + 1, r, r);
        let brush = CreateSolidBrush(c.border);
        FrameRgn(dc, region, brush, 1, 1);
        DeleteObject(brush as _);
        DeleteObject(region as _);
    }
    SetBkMode(dc, TRANSPARENT as _);
    text(dc, &f.caption, c.accent, &p.caption, l.caption, DT_LEFT);
    text(dc, f.big(p.numeric), c.text, &p.big, l.big, DT_LEFT);
    text(dc, &f.unit, c.muted, &p.unit, l.unit, DT_LEFT);
    text(dc, &f.small, c.muted, &p.aside, l.aside, DT_RIGHT);
    if p.bar.is_some() {
        fill(dc, l.bar, c.track);
        let width = l.bar.right - l.bar.left;
        // Something shows for any DPI, however low.
        let filled = ((width as f32 * st.bar.now).round() as i32).clamp(scaled(f.dpi, 3.0).min(width), width);
        fill(dc, rect(l.bar.left, l.bar.top, l.bar.left + filled, l.bar.bottom), c.accent);
    }
    text(dc, &f.small, c.muted, &p.detail, l.detail, DT_LEFT);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rc: RECT = zeroed();
            GetClientRect(hwnd, &mut rc);
            // Drawn off screen, then copied in one go: no flicker when it changes in place.
            let mem = CreateCompatibleDC(dc);
            let bitmap = CreateCompatibleBitmap(dc, rc.right, rc.bottom);
            let old_bitmap = SelectObject(mem, bitmap as _);
            let old_font = SelectObject(mem, GetStockObject(DEFAULT_GUI_FONT));
            STATE.with(|s| {
                if let Ok(guard) = s.try_borrow() {
                    if let Some(st) = guard.as_ref() {
                        paint(mem, st);
                    }
                }
            });
            BitBlt(dc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old_font);
            SelectObject(mem, old_bitmap);
            DeleteObject(bitmap as _);
            DeleteDC(mem);
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_TIMER if wparam == TIMER_HIDE => {
            hide(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_FRAME => {
            frame(hwnd);
            0
        }
        // Sized for its screen already, in `show`.
        WM_DPICHANGED => 0,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::{Popup, Why};

    #[test]
    fn dpi_popup_fills_the_bar_up_to_the_device_maximum() {
        let p = Popup::dpi(1600, 1600, Some(26000), Some((2, 5)), "Basilisk V3");
        assert_eq!((p.big.as_str(), p.unit.as_str(), p.aside.as_str()), ("1600", "DPI", "Stage 2 of 5"));
        assert!((p.bar.unwrap() - 1600.0 / 26000.0).abs() < 1e-6);
        assert_eq!(p.detail, "Basilisk V3");
        let p = Popup::dpi(1600, 800, None, None, "Mouse");
        assert_eq!(p.big, "1600 × 800");
        assert_eq!((p.bar, p.aside.as_str()), (None, ""));
        assert_eq!(Popup::dpi(30000, 30000, Some(26000), None, "").bar, Some(1.0));
    }

    #[test]
    fn one_profile_popup_for_several_devices() {
        let names = ["Gaming".to_string(), "Gaming".to_string()];
        let p = Popup::profile(&names, &["Mouse".into(), "Keyboard".into()], Why::Auto("cs2.exe"));
        assert_eq!((p.caption.as_str(), p.big.as_str(), p.detail.as_str()), ("Auto · cs2.exe", "Gaming", "Mouse, Keyboard"));
        let many: Vec<String> = ["Basilisk V3", "BlackWidow V4 75%", "Firefly V2 Pro"].map(String::from).to_vec();
        let p = Popup::profile(&["Default".into()], &many, Why::Back("cs2.exe"));
        assert_eq!((p.caption.as_str(), p.detail.as_str()), ("Auto · back from cs2.exe", "3 devices"));
        assert_eq!(Popup::profile(&["Work".into()], &["Mouse".into()], Why::Menu).caption, "Profile");
        assert_eq!(Popup::profile(&["A".into(), "B".into()], &[], Why::Menu).big, "A · B");
    }

    #[test]
    fn onboard_popup_says_the_slot_out_of_how_many() {
        let p = Popup::onboard(2, 4, "Basilisk V3", Some((1600, 1600)));
        assert_eq!((p.big.as_str(), p.unit.as_str(), p.aside.as_str()), ("Profile 2", "of 4", "1600 DPI"));
        assert_eq!(Popup::onboard(5, 0, "Mouse", None).unit, "");
        assert_eq!(Popup::onboard(5, 4, "Mouse", None).unit, "");
    }
}
