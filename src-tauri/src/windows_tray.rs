use std::{ffi::c_void, path::PathBuf};

use raw_window_handle::HasWindowHandle;
use tauri::{AppHandle, Manager};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD,
                NIM_DELETE, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
                DestroyMenu, DestroyWindow, GetAncestor, GetCursorPos, GetSystemMetrics,
                GetWindowLongPtrW, LoadIconW, LoadImageW, PostMessageW, RegisterClassW,
                SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TrackPopupMenu, CREATESTRUCTW,
                GA_ROOT, GWLP_USERDATA, HICON, HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE,
                LR_LOADFROMFILE, MF_SEPARATOR, MF_STRING, SM_CYSCREEN, SW_SHOWNORMAL,
                TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN,
                WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_NCCREATE, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
            },
        },
    },
};

const TRAY_UID: u32 = 1;
const WM_TRAYICON: u32 = WM_APP + 0x51;
const MENU_SHOW_MAIN: usize = 1001;
const MENU_QUIT: usize = 1002;

pub struct WindowsTray {
    hwnd: HWND,
    state: *mut WindowsTrayState,
}

struct WindowsTrayState {
    app: AppHandle,
    hicon: HICON,
    owns_icon: bool,
}

// 这个对象只保存 Win32 句柄和 Tauri AppHandle，生命周期由 Tauri state 托管。
unsafe impl Send for WindowsTray {}
unsafe impl Sync for WindowsTray {}

impl Drop for WindowsTray {
    fn drop(&mut self) {
        unsafe {
            let mut nid = notify_icon_data(self.hwnd);
            let _ = Shell_NotifyIconW(NIM_DELETE, &mut nid);
            let _ = DestroyWindow(self.hwnd);

            if !self.state.is_null() {
                let state = Box::from_raw(self.state);
                if state.owns_icon && !state.hicon.is_invalid() {
                    let _ = DestroyIcon(state.hicon);
                }
            }
        }
    }
}

pub fn install(app: AppHandle) -> Result<WindowsTray, String> {
    unsafe {
        let hmodule = GetModuleHandleW(PCWSTR::null()).map_err(|e| e.to_string())?;
        let hinstance = HINSTANCE(hmodule.0);
        let class_name = wide("MiniTodoWindowsTrayWindow");
        let wnd_class = WNDCLASSW {
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            lpfnWndProc: Some(tray_wnd_proc),
            ..Default::default()
        };
        RegisterClassW(&wnd_class);

        let (hicon, owns_icon) = load_tray_icon();
        let state = Box::into_raw(Box::new(WindowsTrayState {
            app,
            hicon,
            owns_icon,
        }));

        let hwnd = match CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinstance,
            Some(state.cast::<c_void>()),
        ) {
            Ok(hwnd) => hwnd,
            Err(e) => {
                cleanup_icon(hicon, owns_icon);
                drop(Box::from_raw(state));
                return Err(e.to_string());
            }
        };

        if let Err(e) = register_tray_icon(hwnd, hicon) {
            let _ = DestroyWindow(hwnd);
            cleanup_icon(hicon, owns_icon);
            drop(Box::from_raw(state));
            return Err(e);
        }

        Ok(WindowsTray { hwnd, state })
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let state = (*create).lpCreateParams as *mut WindowsTrayState;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return LRESULT(1);
        }
        WM_TRAYICON => {
            let event = tray_event_from_lparam(lparam);
            if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                show_tray_menu(hwnd);
                return LRESULT(0);
            }
        }
        WM_CONTEXTMENU => {
            show_tray_menu(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return LRESULT(0);
        }
        _ => {}
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let mut cursor = POINT { x: 0, y: 0 };
    if GetCursorPos(&mut cursor).is_err() {
        return;
    }

    let menu = match CreatePopupMenu() {
        Ok(menu) => menu,
        Err(_) => return,
    };

    let show_label = wide("展开界面");
    let quit_label = wide("退出");
    let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW_MAIN, PCWSTR(show_label.as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit_label.as_ptr()));

    let vertical_align = if is_cursor_in_upper_half(cursor) {
        TPM_TOPALIGN
    } else {
        TPM_BOTTOMALIGN
    };

    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | vertical_align | TPM_RIGHTBUTTON | TPM_RETURNCMD,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        None,
    )
    .0 as usize;
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);

    let state = get_state(hwnd);
    if state.is_null() {
        return;
    }

    match command {
        MENU_SHOW_MAIN => show_main_window_from_tray(&(*state).app),
        MENU_QUIT => (*state).app.exit(0),
        _ => {}
    }
}

unsafe fn is_cursor_in_upper_half(cursor: POINT) -> bool {
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    if !monitor.is_invalid() {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let middle_y = info.rcMonitor.top + (info.rcMonitor.bottom - info.rcMonitor.top) / 2;
            return cursor.y < middle_y;
        }
    }

    let screen_height = GetSystemMetrics(SM_CYSCREEN);
    screen_height > 0 && cursor.y < screen_height / 2
}

unsafe fn register_tray_icon(hwnd: HWND, hicon: HICON) -> Result<(), String> {
    let mut nid = notify_icon_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = hicon;
    write_tip(&mut nid.szTip, "Mini Todo");

    if !Shell_NotifyIconW(NIM_ADD, &mut nid).as_bool() {
        return Err(std::io::Error::last_os_error().to_string());
    }

    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = Shell_NotifyIconW(NIM_SETVERSION, &mut nid);

    Ok(())
}

unsafe fn notify_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_UID,
        ..Default::default()
    }
}

fn tray_event_from_lparam(lparam: LPARAM) -> u32 {
    let raw = lparam.0 as u32;
    let low_word = raw & 0xffff;

    if low_word == WM_CONTEXTMENU || low_word == WM_RBUTTONUP {
        low_word
    } else {
        raw
    }
}

unsafe fn get_state(hwnd: HWND) -> *mut WindowsTrayState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowsTrayState
}

fn load_tray_icon() -> (HICON, bool) {
    for path in icon_candidates() {
        if !path.exists() {
            continue;
        }

        let wide_path = wide_path(&path);
        if let Ok(handle) = unsafe {
            LoadImageW(
                HINSTANCE::default(),
                PCWSTR(wide_path.as_ptr()),
                IMAGE_ICON,
                0,
                0,
                LR_LOADFROMFILE | LR_DEFAULTSIZE,
            )
        } {
            return (HICON(handle.0), true);
        }
    }

    if let Ok(hmodule) = unsafe { GetModuleHandleW(PCWSTR::null()) } {
        if let Ok(icon) = unsafe { LoadIconW(HINSTANCE(hmodule.0), IDI_APPLICATION) } {
            return (icon, false);
        }
    }

    match unsafe { LoadIconW(HINSTANCE::default(), IDI_APPLICATION) } {
        Ok(icon) => (icon, false),
        Err(_) => (HICON::default(), false),
    }
}

unsafe fn cleanup_icon(hicon: HICON, owns_icon: bool) {
    if owns_icon && !hicon.is_invalid() {
        let _ = DestroyIcon(hicon);
    }
}

fn icon_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(current_dir) = std::env::current_dir() {
        paths.push(current_dir.join("icons").join("icon.ico"));
        paths.push(current_dir.join("src-tauri").join("icons").join("icon.ico"));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join("icons").join("icon.ico"));
        }
    }

    paths
}

fn show_main_window_from_tray(app: &AppHandle) {
    if let Some(webview_window) = app.get_webview_window("main") {
        let _ = webview_window.unminimize();
        if let Ok(handle) = webview_window.window_handle() {
            if let raw_window_handle::RawWindowHandle::Win32(win32_handle) = handle.as_raw() {
                let hwnd = HWND(win32_handle.hwnd.get() as *mut _);
                let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
                let target = if root.0.is_null() { hwnd } else { root };
                unsafe {
                    let _ = ShowWindow(target, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(target);
                }
            }
        }
        let _ = webview_window.show();
        let _ = webview_window.set_focus();
    }
}

fn write_tip(target: &mut [u16], text: &str) {
    let encoded = wide(text);
    for (index, code) in encoded
        .iter()
        .copied()
        .take(target.len().saturating_sub(1))
        .enumerate()
    {
        target[index] = code;
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_path(path: &PathBuf) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
