//! How the tray looks: its icon follows the taskbar's light or dark theme and the screen's
//! scale, and its menu turns dark when Windows apps are dark.
//!
//! The process is PerMonitorV2 (see `app.manifest`), so sizes here are real pixels.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use windows_sys::core::PCSTR;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Resource ids from `app.rc`: a white glyph for a dark taskbar, a dark one for a light taskbar.
const ICON_ON_DARK: u16 = 2;
const ICON_ON_LIGHT: u16 = 3;
/// The first build where the uxtheme ordinals below mean what they mean here (1903).
const DARK_MENU_BUILD: u32 = 18362;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A DWORD under HKCU\...\Themes\Personalize; `None` when it isn't there (older Windows).
fn personalize(name: &str) -> Option<u32> {
    let mut data: u32 = 0;
    let mut size = 4u32;
    let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), wide(name).as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut data as *mut u32 as *mut c_void, &mut size)
    };
    (ok == ERROR_SUCCESS).then_some(data)
}

/// The taskbar, hence the notification area, is light. Windows' own setting for it is apart
/// from the apps' one ("dark Windows, light apps" is a choice), and defaults to dark.
pub fn taskbar_light() -> bool {
    personalize("SystemUsesLightTheme") == Some(1)
}

fn reg_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 64];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let ok = unsafe {
        RegGetValueW(root, wide(path).as_ptr(), wide(name).as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size)
    };
    if ok != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Windows' build number (e.g. 22631), 0 if unknown.
fn windows_build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber")
            .and_then(|b| b.trim().parse().ok())
            .unwrap_or(0)
    })
}

/// An export of uxtheme.dll by ordinal. These have no name: they are what Explorer, Notepad,
/// Chromium and others use for dark menus, unchanged since Windows 10 1903. `None` on a build
/// before that, or if one is missing, and the menu stays light.
fn uxtheme(ordinal: u16) -> Option<unsafe extern "system" fn() -> isize> {
    if windows_build() < DARK_MENU_BUILD {
        return None;
    }
    static LIB: OnceLock<usize> = OnceLock::new();
    let lib = *LIB.get_or_init(|| unsafe { LoadLibraryExW(wide("uxtheme.dll").as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) as usize });
    if lib == 0 {
        return None;
    }
    // An ordinal goes in the low word of the name pointer (MAKEINTRESOURCEA).
    unsafe { GetProcAddress(lib as HMODULE, ordinal as usize as PCSTR) }
}

/// Let the tray's menu follow the apps' theme: dark when Windows apps are dark. Called at
/// start for the window that owns the menu, and again when the theme changes.
pub fn dark_menus(hwnd: HWND) {
    // SAFETY: each pointer comes from uxtheme with the signature these ordinals have had
    // since 1903 (`uxtheme` refuses older builds).
    unsafe {
        if let Some(f) = uxtheme(135) {
            // SetPreferredAppMode(AllowDark): dark only while apps are dark.
            let set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_preferred_app_mode(1);
        }
        if let Some(f) = uxtheme(133) {
            let allow_dark_mode_for_window: unsafe extern "system" fn(HWND, BOOL) -> BOOL = std::mem::transmute(f);
            allow_dark_mode_for_window(hwnd, TRUE);
        }
        if let Some(f) = uxtheme(104) {
            // RefreshImmersiveColorPolicyState: read the apps' theme again after a change.
            let refresh: unsafe extern "system" fn() = std::mem::transmute(f);
            refresh();
        }
        if let Some(f) = uxtheme(136) {
            // FlushMenuThemes: the next menu is drawn with the theme now in force.
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

/// True when a WM_SETTINGCHANGE says the light/dark theme changed.
pub fn is_theme_change(lparam: LPARAM) -> bool {
    if lparam == 0 {
        return false;
    }
    let want: Vec<u16> = "ImmersiveColorSet".encode_utf16().collect();
    let text = lparam as *const u16;
    // SAFETY: WM_SETTINGCHANGE's lparam, when set, is a NUL-terminated string; it is read up
    // to its NUL or one character past the name.
    unsafe {
        let mut len = 0;
        while len <= want.len() && *text.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(text, len) == want.as_slice()
    }
}

/// The tray icon for the taskbar's theme, at the small-icon size of the screen `hwnd` is on
/// (16 px at 100 %, 24 at 150 %, 32 at 200 %). The caller owns it and destroys it.
pub fn tray_icon(hwnd: HWND) -> HICON {
    unsafe {
        let dpi = match GetDpiForWindow(hwnd) {
            0 => 96,
            d => d,
        };
        let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi);
        let id = if taskbar_light() { ICON_ON_LIGHT } else { ICON_ON_DARK };
        let icon = LoadImageW(GetModuleHandleW(null()), id as usize as *const u16, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
        if icon.is_null() {
            // A copy, so it can be destroyed like the others.
            CopyIcon(LoadIconW(null_mut(), IDI_APPLICATION))
        } else {
            icon as HICON
        }
    }
}
