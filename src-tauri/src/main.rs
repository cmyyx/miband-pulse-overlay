#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bluest::{btuuid::bluetooth_uuid_from_u16, Adapter, Device, Uuid};
use futures_lite::stream::StreamExt;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};
use tokio::sync::{watch, RwLock};
use warp::Filter;

const HRS_UUID: Uuid = bluetooth_uuid_from_u16(0x180D);
const HRM_UUID: Uuid = bluetooth_uuid_from_u16(0x2A37);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartRate {
    value: u16,
    sensor_contact: Option<bool>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartRateStats {
    value: u16,
    max: u16,
    avg: u16,
    sensor_contact: Option<bool>,
}

struct AppState {
    heart_rate_tx: watch::Sender<HeartRate>,
    web_server_running: Arc<RwLock<bool>>,
    is_pinned: Arc<Mutex<bool>>,
    is_settings_open: Arc<Mutex<bool>>,
    auto_hide: Arc<Mutex<bool>>,
    auto_hidden: Arc<Mutex<bool>>,
    is_always_on_top: Arc<Mutex<bool>>,
    /// 托盘菜单项（用于动态更新文字）
    pin_menu_item: Arc<Mutex<Option<MenuItem<tauri::Wry>>>>,
    settings_menu_item: Arc<Mutex<Option<MenuItem<tauri::Wry>>>>,
    obs_menu_item: Arc<Mutex<Option<MenuItem<tauri::Wry>>>>,
    auto_hide_menu_item: Arc<Mutex<Option<CheckMenuItem<tauri::Wry>>>>,
}

// ── Windows 窗口边框剥离（使用窗口子类化拦截非客户区激活消息） ─────────────────────────────────

#[cfg(target_os = "windows")]
mod border_strip {
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    static ORIGINAL_WNDPROC: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
    static SUBCLASS_INSTALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            // 拦截非客户区激活和绘制消息，阻止标题栏和按钮显示
            WM_NCACTIVATE | WM_NCPAINT => {
                LRESULT(1) // 返回 1 表示"已处理"，阻止默认绘制
            }
            _ => {
                // 调用原始窗口过程
                let orig = ORIGINAL_WNDPROC.load(Ordering::Relaxed);
                if !orig.is_null() {
                    CallWindowProcW(
                        Some(std::mem::transmute(orig)),
                        hwnd,
                        msg,
                        wparam,
                        lparam,
                    )
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
        }
    }

    pub unsafe fn install_subclass(hwnd: HWND) {
        // 只安装一次，避免重复安装导致栈溢出
        if SUBCLASS_INSTALLED.load(Ordering::Relaxed) {
            return;
        }
        SUBCLASS_INSTALLED.store(true, Ordering::Relaxed);
        let current = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        ORIGINAL_WNDPROC.store(current as *mut (), Ordering::Relaxed);
        let _ = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_proc as *const () as usize as isize);
    }
}

#[cfg(target_os = "windows")]
unsafe fn strip_window_borders(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, HWND_TOP,
        SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_BORDER, WS_CAPTION,
        WS_DLGFRAME, WS_EX_CLIENTEDGE, WS_EX_STATICEDGE, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };

    let style = GetWindowLongW(hwnd, GWL_STYLE);
    let new_style = style
        & !(WS_THICKFRAME.0 | WS_BORDER.0 | WS_DLGFRAME.0 | WS_CAPTION.0 | WS_SYSMENU.0
            | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0) as i32;
    let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style);

    // 移除扩展边框样式
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
    let new_ex_style =
        ex_style & !(WS_EX_WINDOWEDGE.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0) as i32;
    let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, new_ex_style);

    let _ = SetWindowPos(
        hwnd,
        HWND_TOP,
        0, 0, 0, 0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
    );
}

#[cfg(target_os = "windows")]
unsafe fn disable_dwm_border(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY,
        DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
    };
    // 禁用非客户区渲染
    let policy = DWMNCRP_DISABLED;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY,
        &policy as *const _ as *const _,
        std::mem::size_of_val(&policy) as u32,
    );
    // 设置边框颜色为无
    let color = DWMWA_COLOR_NONE;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        &color as *const _ as *const _,
        std::mem::size_of_val(&color) as u32,
    );
}

#[cfg(target_os = "windows")]
unsafe fn strip_all_borders(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;

    // Tauri 2 中 hwnd() 返回 Result<HWND, Error>
    let hwnd_result = window.hwnd();
    if hwnd_result.is_err() {
        return;
    }
    let hwnd = hwnd_result.unwrap();
    let parent_hwnd = HWND(hwnd.0 as *mut _);

    // 安装窗口子类化，拦截非客户区绘制消息
    border_strip::install_subclass(parent_hwnd);

    // 剥离父窗口边框
    strip_window_borders(parent_hwnd);
    disable_dwm_border(parent_hwnd);
    // 注意：不处理 WebView2 子窗口，保持其 WS_CHILD 风格以确保鼠标输入正常
}

// ── 窗口位置持久化 ────────────────────────────────────────────────

fn position_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let data_dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&data_dir);
    Some(data_dir.join("window-pos.json"))
}

fn load_window_position(app: &AppHandle) -> Option<(i32, i32)> {
    let path = position_path(app)?;
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let x = parsed["x"].as_i64()? as i32;
    let y = parsed["y"].as_i64()? as i32;
    Some((x, y))
}

fn save_window_position(app: &AppHandle, x: i32, y: i32) {
    if let Some(path) = position_path(app) {
        if let Ok(json) = serde_json::to_string(&serde_json::json!({"x": x, "y": y})) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn center_window(window: &tauri::WebviewWindow) {
    if let Ok(Some(monitor)) = window.primary_monitor() {
        let screen_size = monitor.size();
        let window_size = window
            .outer_size()
            .unwrap_or(tauri::PhysicalSize { width: 320, height: 180 });
        let x = ((screen_size.width as i32) - (window_size.width as i32)) / 2;
        let y = ((screen_size.height as i32) - (window_size.height as i32)) / 2;
        let _ = window.set_position(tauri::PhysicalPosition::new(x.max(0), y.max(0)));
    }
}

// ── Tauri 命令 ─────────────────────────────────────────────────────

#[tauri::command]
async fn toggle_click_through(window: tauri::WebviewWindow, ignore: bool) -> Result<(), String> {
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn set_always_on_top(window: tauri::WebviewWindow, always: bool, state: State<'_, AppState>) -> Result<(), String> {
    let mut current = state.is_always_on_top.lock().unwrap();
    if *current == always {
        // 状态相同但需要强制刷新 Z 序：先移除再重新插入 topmost 层
        window.set_always_on_top(!always)
            .map_err(|e| e.to_string())?;
    }
    window
        .set_always_on_top(always)
        .map_err(|e| e.to_string())?;
    *current = always;
    Ok(())
}

#[tauri::command]
async fn resize_window(window: tauri::WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }))
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_dragging(window: tauri::WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn hide_window(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    *state.auto_hidden.lock().unwrap() = true;
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
async fn show_window(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    *state.auto_hidden.lock().unwrap() = false;
    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    Ok(())
}

#[tauri::command]
async fn toggle_web_server(
    app: AppHandle,
    state: State<'_, AppState>,
    port: Option<u16>,
) -> Result<bool, String> {
    let mut running = state.web_server_running.write().await;
    if *running {
        *running = false;
        Ok(false)
    } else {
        let rx = state.heart_rate_tx.subscribe();
        let running_flag = state.web_server_running.clone();
        let port = port.unwrap_or(3030);

        // 从配置文件读取统计周期
        let max_history_seconds = settings_path(&app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("maxHistorySeconds").and_then(|v| v.as_u64()))
            .unwrap_or(60);

        tokio::spawn(async move {
            *running_flag.write().await = true;
            if let Err(e) = start_web_server(rx, port, max_history_seconds).await {
                eprintln!("Web server error: {e:?}");
            }
            *running_flag.write().await = false;
        });

        Ok(true)
    }
}

#[tauri::command]
async fn reset_window_position(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    center_window(&window);
    if let Ok(pos) = window.outer_position() {
        save_window_position(&app, pos.x, pos.y);
    }
    Ok(())
}

#[tauri::command]
fn open_data_dir(app: AppHandle) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let path_str = data_dir.to_string_lossy().to_string();
    if std::process::Command::new("explorer")
        .arg(&data_dir)
        .spawn()
        .is_err()
    {
        return Err(format!("无法打开目录: {}", path_str));
    }
    Ok(path_str)
}

fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let data_dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&data_dir);
    Some(data_dir.join("settings.json"))
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<String, String> {
    let path = settings_path(&app).ok_or("无法获取数据目录")?;
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())
    } else {
        Ok("{}".to_string())
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: String) -> Result<(), String> {
    let path = settings_path(&app).ok_or("无法获取数据目录")?;
    std::fs::write(&path, &settings).map_err(|e| e.to_string())
}

/// 前端通知后端：pin 状态变化（由 togglePin() 调用）
#[tauri::command]
async fn notify_pin_state(
    state: State<'_, AppState>,
    app: AppHandle,
    pinned: bool,
) -> Result<(), String> {
    *state.is_pinned.lock().unwrap() = pinned;
    update_pin_menu(&app, pinned);
    Ok(())
}

/// 前端通知后端：设置面板状态变化
#[tauri::command]
async fn notify_settings_toggled(
    state: State<'_, AppState>,
    app: AppHandle,
    open: bool,
) -> Result<(), String> {
    *state.is_settings_open.lock().unwrap() = open;
    update_settings_menu(&app, open);
    Ok(())
}

/// 前端调用：在系统默认浏览器中打开外部链接
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

// ── 菜单文字更新辅助函数 ──────────────────────────────────────────

fn update_pin_menu(app: &AppHandle, pinned: bool) {
    let state = app.state::<AppState>();
    let mi = state.pin_menu_item.lock().unwrap();
    if let Some(ref item) = *mi {
        let text = if pinned { "取消固定位置" } else { "固定位置" };
        let _ = item.set_text(text);
    }
}

fn update_settings_menu(app: &AppHandle, open: bool) {
    let state = app.state::<AppState>();
    let mi = state.settings_menu_item.lock().unwrap();
    if let Some(ref item) = *mi {
        let text = if open { "关闭设置面板" } else { "打开设置面板" };
        let _ = item.set_text(text);
    }
}

fn update_obs_menu(app: &AppHandle, running: bool) {
    let state = app.state::<AppState>();
    let mi = state.obs_menu_item.lock().unwrap();
    if let Some(ref item) = *mi {
        let text = if running {
            "关闭 OBS Server"
        } else {
            "打开 OBS Server"
        };
        let _ = item.set_text(text);
    }
}

fn update_auto_hide_menu(app: &AppHandle, enabled: bool) {
    let state = app.state::<AppState>();
    let mi = state.auto_hide_menu_item.lock().unwrap();
    if let Some(ref item) = *mi {
        let _ = item.set_checked(enabled);
    }
}

// ── "关于"窗口 ────────────────────────────────────────────────────

const ABOUT_WINDOW_LABEL: &str = "about";

/// 打开"关于"窗口：已存在则置顶聚焦，否则创建。
fn open_about_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(ABOUT_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }

    let result = tauri::WebviewWindowBuilder::new(
        app,
        ABOUT_WINDOW_LABEL,
        tauri::WebviewUrl::App("about.html".into()),
    )
    .title("关于 MiBand Pulse Overlay")
    .inner_size(460.0, 400.0)
    .min_inner_size(420.0, 360.0)
    .resizable(false)
    .center()
    .build();

    if let Err(e) = result {
        eprintln!("Failed to create about window: {e:?}");
    }
}

// ── Web 服务器 ─────────────────────────────────────────────────────

async fn start_web_server(
    rx: watch::Receiver<HeartRate>,
    port: u16,
    max_history_seconds: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let index_html = include_str!("../../web/index.html").to_string();

    struct SharedState {
        stats: HeartRateStats,
        last_update: Instant,
    }

    let state: Arc<std::sync::Mutex<SharedState>> =
        Arc::new(std::sync::Mutex::new(SharedState {
            stats: HeartRateStats {
                value: 0,
                max: 0,
                avg: 0,
                sensor_contact: None,
            },
            last_update: Instant::now(),
        }));

    let notify = Arc::new(tokio::sync::Notify::new());

    // 后台任务：持续更新历史记录和统计数据
    {
        let state = state.clone();
        let notify = notify.clone();
        let mut rx = rx.clone();
        tokio::spawn(async move {
            let mut history: Vec<(Instant, u16)> = Vec::new();
            let max_history = std::time::Duration::from_secs(max_history_seconds);
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let hr = rx.borrow().clone();
                let now = Instant::now();
                history.push((now, hr.value));
                // 清理过期数据
                let cutoff = now - max_history;
                history.retain(|(t, _)| *t > cutoff);
                // 计算统计
                let max = history.iter().map(|(_, v)| *v).max().unwrap_or(hr.value);
                let avg = if history.is_empty() {
                    hr.value
                } else {
                    (history.iter().map(|(_, v)| *v as u64).sum::<u64>()
                        / history.len() as u64) as u16
                };
                if let Ok(mut s) = state.lock() {
                    s.stats = HeartRateStats {
                        value: hr.value,
                        max,
                        avg,
                        sensor_contact: hr.sensor_contact,
                    };
                    s.last_update = now;
                }
                notify.notify_waiters();
            }
        });
    }

    let root = warp::path::end().map(move || warp::reply::html(index_html.clone()));

    let heartrate = {
        let state = state.clone();
        let notify = notify.clone();
        warp::path!("heartrate").then(move || {
            let state = state.clone();
            let notify = notify.clone();
            async move {
                let need_wait = {
                    let s = state.lock().unwrap();
                    s.last_update.elapsed() > std::time::Duration::from_secs(5)
                };
                if need_wait {
                    // 数据不新鲜，等待新数据（最多 5 秒）
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        notify.notified(),
                    )
                    .await;
                }

                let s = state.lock().unwrap();
                let elapsed = s.last_update.elapsed();
                // 超过 10 秒无数据视为断开，返回全零
                if elapsed > std::time::Duration::from_secs(10) {
                    warp::reply::json(&HeartRateStats {
                        value: 0,
                        max: 0,
                        avg: 0,
                        sensor_contact: None,
                    })
                } else {
                    warp::reply::json(&s.stats)
                }
            }
        })
    };

    let socket_addr: SocketAddr = ([127, 0, 0, 1], port).into();
    println!("Web server listening at http://{socket_addr}");

    warp::serve(warp::get().and(root.or(heartrate)))
        .run(socket_addr)
        .await;

    Ok(())
}

// ── 主入口 ─────────────────────────────────────────────────────────

fn main() {
    let (heart_rate_tx, _) = watch::channel(HeartRate {
        value: 0,
        sensor_contact: None,
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            heart_rate_tx,
            web_server_running: Arc::new(RwLock::new(false)),
            is_pinned: Arc::new(Mutex::new(false)),
            is_settings_open: Arc::new(Mutex::new(false)),
            auto_hide: Arc::new(Mutex::new(false)),
            auto_hidden: Arc::new(Mutex::new(false)),
            is_always_on_top: Arc::new(Mutex::new(false)),
            pin_menu_item: Arc::new(Mutex::new(None)),
            settings_menu_item: Arc::new(Mutex::new(None)),
            obs_menu_item: Arc::new(Mutex::new(None)),
            auto_hide_menu_item: Arc::new(Mutex::new(None)),
        })
        .setup(|app| {
            // ── 系统托盘菜单 ──────────────────────────────────────
            let pin_item = MenuItem::with_id(app, "toggle-pin", "固定位置", true, None::<&str>)?;
            let obs_item =
                MenuItem::with_id(app, "toggle-obs", "打开 OBS Server", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "toggle-settings", "打开设置面板", true, None::<&str>)?;
            let reset_pos =
                MenuItem::with_id(app, "reset-position", "重置窗口位置", true, None::<&str>)?;
            let open_data = MenuItem::with_id(app, "open-data-dir", "打开数据目录", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
            let about_separator = PredefinedMenuItem::separator(app)?;
            let about_item = MenuItem::with_id(
                app,
                "open-about",
                "关于 / About",
                true,
                None::<&str>,
            )?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

            // 从持久化设置恢复 auto_hide 状态
            let initial_auto_hide = app
                .path()
                .app_data_dir()
                .ok()
                .and_then(|p| {
                    let _ = std::fs::create_dir_all(&p);
                    let settings_file = p.join("settings.json");
                    std::fs::read_to_string(&settings_file).ok()
                })
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("autoHide").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            {
                let state = app.state::<AppState>();
                *state.auto_hide.lock().unwrap() = initial_auto_hide;
            }
            let auto_hide_item = CheckMenuItem::with_id(
                app,
                "toggle-auto-hide",
                "自动隐藏",
                true,
                initial_auto_hide,
                None::<&str>,
            )?;

            // 保存菜单项引用到 AppState，用于后续动态更新文字
            {
                let state = app.state::<AppState>();
                *state.pin_menu_item.lock().unwrap() = Some(pin_item.clone());
                *state.obs_menu_item.lock().unwrap() = Some(obs_item.clone());
                *state.settings_menu_item.lock().unwrap() = Some(settings_item.clone());
                *state.auto_hide_menu_item.lock().unwrap() = Some(auto_hide_item.clone());
            }

            let menu = Menu::with_items(
                app,
                &[
                    &pin_item, &obs_item, &settings_item, &auto_hide_item, &reset_pos, &open_data,
                    &show_i, &about_separator, &about_item, &quit_i,
                ],
            )?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("MiBand Pulse Overlay")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "toggle-pin" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let state = app.state::<AppState>();
                            let mut pinned = state.is_pinned.lock().unwrap();
                            *pinned = !*pinned;
                            update_pin_menu(app, *pinned);
                            let _ = app.emit("pin-toggled", ());
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "toggle-obs" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = handle.state::<AppState>();
                            let mut running = state.web_server_running.write().await;
                            if *running {
                                *running = false;
                            } else {
                                let rx = state.heart_rate_tx.subscribe();
                                let running_flag = state.web_server_running.clone();
                                let max_history_seconds = settings_path(&handle)
                                    .and_then(|p| std::fs::read_to_string(p).ok())
                                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                    .and_then(|v| v.get("maxHistorySeconds").and_then(|v| v.as_u64()))
                                    .unwrap_or(60);
                                tokio::spawn(async move {
                                    *running_flag.write().await = true;
                                    if let Err(e) = start_web_server(rx, 3030, max_history_seconds).await {
                                        eprintln!("Web server error: {e:?}");
                                    }
                                    *running_flag.write().await = false;
                                });
                            }
                            if let Some(window) = handle.get_webview_window("main") {
                                update_obs_menu(&handle, !*running);
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                            let _ = handle.emit("obs-state-changed", !*running);
                        });
                    }
                    "toggle-settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let state = app.state::<AppState>();
                            let mut is_open = state.is_settings_open.lock().unwrap();
                            *is_open = !*is_open;
                            update_settings_menu(app, *is_open);

                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                        let _ = app.emit("toggle-settings", ());
                    }
                    "toggle-auto-hide" => {
                        let state = app.state::<AppState>();
                        let mut ah = state.auto_hide.lock().unwrap();
                        *ah = !*ah;
                        update_auto_hide_menu(app, *ah);
                        let _ = app.emit("toggle-auto-hide", *ah);
                        if !*ah {
                            *state.auto_hidden.lock().unwrap() = false;
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                    "open-data-dir" => {
                        let app_handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Ok(data_dir) = app_handle.path().app_data_dir() {
                                let _ = std::process::Command::new("explorer")
                                    .arg(&data_dir)
                                    .spawn();
                            }
                        });
                    }
                    "reset-position" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let app_handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                center_window(&window);
                                if let Ok(pos) = window.outer_position() {
                                    save_window_position(&app_handle, pos.x, pos.y);
                                }
                            });
                        }
                    }
                    "open-about" => {
                        open_about_window(app);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            // ── 窗口初始化 ────────────────────────────────────────
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_skip_taskbar(true);

                // 窗口位置：优先恢复保存位置，否则屏幕居中
                let app_handle = app.handle().clone();
                if let Some((x, y)) = load_window_position(&app_handle) {
                    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
                } else {
                    center_window(&window);
                }

                // Windows 边框剥离
                #[cfg(target_os = "windows")]
                {
                    unsafe {
                        strip_all_borders(&window);
                    }
                }

                // 窗口事件监听
                window.on_window_event({
                    let app_handle = app.handle().clone();
                    move |event| {
                        // 窗口移动时自动保存位置
                        if let tauri::WindowEvent::Moved(position) = event {
                            save_window_position(&app_handle, position.x, position.y);
                        }
                    }
                });

                // 延迟显示窗口，确保边框已完全剥离
                let window_clone = window.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    let _ = window_clone.show();
                });
            }

            // ── 启动 BLE 心率监听 ─────────────────────────────────
            let handle = app.handle().clone();
            let heart_rate_tx = {
                let state = handle.state::<AppState>();
                state.heart_rate_tx.clone()
            };
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    if let Err(e) = start_ble_monitor(handle, heart_rate_tx).await {
                        eprintln!("BLE monitor error: {e:?}");
                    }
                });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            toggle_click_through,
            set_always_on_top,
            resize_window,
            start_dragging,
            hide_window,
            show_window,
            toggle_web_server,
            reset_window_position,
            notify_pin_state,
            notify_settings_toggled,
            open_data_dir,
            load_settings,
            save_settings,
            open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── BLE 心率监听 ──────────────────────────────────────────────────

async fn start_ble_monitor(
    app_handle: AppHandle,
    heart_rate_tx: watch::Sender<HeartRate>,
) -> Result<(), Box<dyn std::error::Error>> {
    let retry_delay = tokio::time::Duration::from_secs(5);
    let scan_timeout = tokio::time::Duration::from_secs(30);

    'outer: loop {
        // 每次循环都重新获取 Adapter，确保睡眠/休眠唤醒后使用有效句柄
        let adapter = match Adapter::default().await {
            Some(a) => a,
            None => {
                eprintln!("BLE: Bluetooth adapter not found, retrying in {retry_delay:?}...");
                tokio::time::sleep(retry_delay).await;
                continue 'outer;
            }
        };

        if let Err(e) = adapter.wait_available().await {
            eprintln!("BLE: wait_available failed: {e:?}, retrying in {retry_delay:?}...");
            tokio::time::sleep(retry_delay).await;
            continue 'outer;
        }

        println!("BLE: Adapter ready");

        // ── 查找设备（先查已连接，再扫描） ──────────────────────
        let device = loop {
            // 检查已连接的设备
            match adapter.connected_devices_with_services(&[HRS_UUID]).await {
                Ok(devices) => {
                    if let Some(device) = devices.into_iter().next() {
                        println!("BLE: Found connected device");
                        break device;
                    }
                }
                Err(e) => {
                    eprintln!("BLE: connected_devices_with_services error: {e:?}, refreshing adapter...");
                    continue 'outer;
                }
            }

            // 扫描新设备
            println!("BLE: Starting scan");
            let mut scan = match adapter.discover_devices(&[HRS_UUID]).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("BLE: discover_devices error: {e:?}, refreshing adapter...");
                    continue 'outer;
                }
            };

            // 带超时等待设备
            match tokio::time::timeout(scan_timeout, scan.next()).await {
                Ok(Some(Ok(device))) => {
                    println!(
                        "BLE: Found device: [{}] {:?}",
                        device,
                        device.name_async().await
                    );
                    break device;
                }
                Ok(Some(Err(e))) => {
                    eprintln!("BLE: Scan error: {e:?}, rescanning...");
                    continue;
                }
                Ok(None) => {
                    eprintln!("BLE: Scan ended with no device, rescanning...");
                    continue;
                }
                Err(_) => {
                    eprintln!("BLE: Scan timeout ({scan_timeout:?}), rescanning...");
                    continue;
                }
            }
        };

        // ── 连接并监听 ──────────────────────────────────────────
        match handle_device(&adapter, &device, &app_handle, &heart_rate_tx).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("BLE: Connection error: {e:?}, reconnecting...");
            }
        }
    }
}

async fn handle_device(
    adapter: &Adapter,
    device: &Device,
    app_handle: &AppHandle,
    heart_rate_tx: &watch::Sender<HeartRate>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !device.is_connected().await {
        println!("Connecting device: {}", device.id());
        adapter.connect_device(device).await?;
    }

    let heart_rate_services = device.discover_services_with_uuid(HRS_UUID).await?;
    let heart_rate_service = heart_rate_services
        .first()
        .ok_or("Device should have one heart rate service at least")?;

    let heart_rate_measurements = heart_rate_service
        .discover_characteristics_with_uuid(HRM_UUID)
        .await?;
    let heart_rate_measurement = heart_rate_measurements
        .first()
        .ok_or(
            "HeartRateService should have one heart rate measurement characteristic at least",
        )?;

    let mut updates = heart_rate_measurement.notify().await?;
    while let Some(Ok(heart_rate)) = updates.next().await {
        let flag = *heart_rate.get(0).ok_or("No flag")?;

        let mut heart_rate_value = *heart_rate.get(1).ok_or("No heart rate u8")? as u16;
        if flag & 0b00001 != 0 {
            heart_rate_value |=
                (*heart_rate.get(2).ok_or("No heart rate u16")? as u16) << 8;
        }

        let mut sensor_contact = None;
        if flag & 0b00100 != 0 {
            sensor_contact = Some(flag & 0b00010 != 0)
        }

        println!(
            "HeartRateValue: {heart_rate_value}, SensorContactDetected: {sensor_contact:?}"
        );

        let hr = HeartRate {
            value: heart_rate_value,
            sensor_contact,
        };

        let _ = heart_rate_tx.send(hr.clone());
        let _ = app_handle.emit("heart-rate-update", hr);
    }
    Err("No longer heart rate notify".into())
}
