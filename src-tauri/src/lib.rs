use serde::Serialize;
use snaphost_core::{
    CURRENT_VERSION, LocalReverseProxy, MergeResult, PortMapping, Profile, SnapHostManager,
    SystemStatus, UpdateInfo, ValidationIssue, check_for_updates, check_port_alive,
    download_and_install_update, validate_hosts,
};
use std::sync::Mutex;
use tauri::{
    ActivationPolicy, AppHandle, Emitter, Manager, State, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

pub struct AppState {
    pub manager: Mutex<SnapHostManager>,
    pub proxy: LocalReverseProxy,
}

#[derive(Serialize)]
pub struct PortMappingStatus {
    pub mapping: PortMapping,
    pub alive: bool,
}

#[derive(Serialize)]
pub struct AppSnapshot {
    pub app_version: String,
    pub profiles: Vec<Profile>,
    pub single_switch_mode: bool,
    pub status: SystemStatus,
    pub preview: MergeResult,
    pub port_statuses: Vec<PortMappingStatus>,
    pub proxy_running: bool,
    pub proxy_port: u16,
}

fn sync_proxy_with_mappings(proxy: &LocalReverseProxy, mappings: &[PortMapping]) {
    proxy.update_mappings(mappings);
    if !mappings.is_empty() && !proxy.is_running() {
        let _ = proxy.start(80);
    }
}

fn build_snapshot_with_lock(state: &AppState) -> Result<AppSnapshot, String> {
    let (profiles, single_switch_mode, status, preview) = {
        let mgr = state.manager.lock().map_err(|e| e.to_string())?;
        (
            mgr.profiles().to_vec(),
            mgr.single_switch_mode(),
            mgr.status(),
            mgr.preview_merge(),
        )
    };

    sync_proxy_with_mappings(&state.proxy, &preview.port_mappings);

    let port_statuses = std::thread::scope(|s| {
        let handles: Vec<_> = preview
            .port_mappings
            .iter()
            .map(|m| {
                let mapping = m.clone();
                s.spawn(move || PortMappingStatus {
                    alive: check_port_alive(mapping.target_ip, mapping.target_port),
                    mapping,
                })
            })
            .collect();

        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect::<Vec<_>>()
    });

    Ok(AppSnapshot {
        app_version: CURRENT_VERSION.to_string(),
        profiles,
        single_switch_mode,
        status,
        preview,
        port_statuses,
        proxy_running: state.proxy.is_running(),
        proxy_port: state.proxy.current_port(),
    })
}

#[tauri::command]
fn get_app_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    build_snapshot_with_lock(&state)
}

#[tauri::command]
fn create_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    content: String,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        mgr.create_profile(name, content)
            .map_err(|e| e.to_string())?;
    }
    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn update_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
    content: String,
    apply_immediately: bool,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        let updated = mgr
            .update_profile(&id, name, content)
            .map_err(|e| e.to_string())?;

        if apply_immediately && updated.enabled {
            mgr.apply_to_system().map_err(|e| e.to_string())?;
        }
    }

    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn delete_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        let was_enabled = mgr.delete_profile(&id).map_err(|e| e.to_string())?;
        if was_enabled {
            mgr.apply_to_system().map_err(|e| e.to_string())?;
        }
    }
    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn toggle_profile(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        mgr.toggle_profile(&id, enabled)
            .map_err(|e| e.to_string())?;
    }
    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn set_switch_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    single_mode: bool,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        mgr.set_single_switch_mode(single_mode)
            .map_err(|e| e.to_string())?;
    }
    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn grant_permission(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    {
        let mgr = state.manager.lock().map_err(|e| e.to_string())?;
        mgr.grant_one_time_permission()
            .map_err(|e| e.to_string())?;
    }
    build_snapshot_with_lock(&state)
}

#[tauri::command]
fn panic_restore(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    {
        let mut mgr = state.manager.lock().map_err(|e| e.to_string())?;
        mgr.restore_original_backup().map_err(|e| e.to_string())?;
    }
    let snapshot = build_snapshot_with_lock(&state)?;
    let _ = refresh_tray_menu(&app);
    Ok(snapshot)
}

#[tauri::command]
fn validate_hosts_text(content: String) -> Vec<ValidationIssue> {
    validate_hosts(&content)
}

#[tauri::command]
fn check_update() -> Result<UpdateInfo, String> {
    check_for_updates()
}

#[tauri::command]
fn install_update(
    app: AppHandle,
    download_url: String,
    asset_name: String,
) -> Result<(), String> {
    download_and_install_update(&download_url, &asset_name)?;
    app.exit(0);
    Ok(())
}

fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<AppState>();
    let mgr = state.manager.lock().unwrap_or_else(|e| e.into_inner());

    let menu = Menu::new(app)?;
    let header = MenuItem::with_id(
        app,
        "header",
        "SnapHost 프로필 전환",
        false,
        None::<&str>,
    )?;
    menu.append(&header)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    for profile in mgr.profiles() {
        let label = if profile.is_system_base {
            format!("{} (기본)", profile.name)
        } else {
            profile.name.clone()
        };
        let item = CheckMenuItem::with_id(
            app,
            format!("toggle:{}", profile.id),
            label,
            true,
            profile.enabled,
            None::<&str>,
        )?;
        menu.append(&item)?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let open_editor = MenuItem::with_id(
        app,
        "open_editor",
        "SnapHost 에디터 열기...",
        true,
        Some("CmdOrCtrl+O"),
    )?;
    let flush_dns = MenuItem::with_id(
        app,
        "flush_dns",
        "DNS 캐시 즉시 초기화",
        true,
        None::<&str>,
    )?;
    let check_upd = MenuItem::with_id(
        app,
        "check_update_menu",
        format!("업데이트 확인 (현재 v{CURRENT_VERSION})..."),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "SnapHost 종료", true, Some("CmdOrCtrl+Q"))?;

    menu.append(&open_editor)?;
    menu.append(&flush_dns)?;
    menu.append(&check_upd)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&quit)?;

    Ok(menu)
}

fn refresh_tray_menu(app: &AppHandle) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id("snaphost-tray") {
        let menu = build_tray_menu(app)?;
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let manager = SnapHostManager::init_default().expect("Failed to initialize SnapHostManager");
    let proxy = LocalReverseProxy::new();
    let initial_preview = manager.preview_merge();
    sync_proxy_with_mappings(&proxy, &initial_preview.port_mappings);

    tauri::Builder::default()
        .manage(AppState {
            manager: Mutex::new(manager),
            proxy,
        })
        .setup(|app| {
            let menu = build_tray_menu(app.handle())?;
            let _tray = TrayIconBuilder::with_id("snaphost-tray")
                .title("⚡")
                .tooltip("SnapHost — Ultra-fast Hosts Switcher")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref();
                    if let Some(profile_id) = id.strip_prefix("toggle:") {
                        let state = app.state::<AppState>();
                        let mut should_refresh = false;
                        if let Ok(mut mgr) = state.manager.lock() {
                            if let Some(p) = mgr.profiles().iter().find(|p| p.id == profile_id) {
                                let next_state = !p.enabled;
                                if let Ok(merged) = mgr.toggle_profile(profile_id, next_state) {
                                    sync_proxy_with_mappings(&state.proxy, &merged.port_mappings);
                                    should_refresh = true;
                                }
                            }
                        }
                        if should_refresh {
                            let _ = refresh_tray_menu(app);
                            let _ = app.emit("profiles-updated", ());
                        }
                    } else if id == "open_editor" {
                        #[cfg(target_os = "macos")]
                        let _ = app.set_activation_policy(ActivationPolicy::Regular);
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                    } else if id == "flush_dns" {
                        let _ = snaphost_core::flush_dns_cache();
                    } else if id == "check_update_menu" {
                        #[cfg(target_os = "macos")]
                        let _ = app.set_activation_policy(ActivationPolicy::Regular);
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.unminimize();
                            let _ = win.set_focus();
                        }
                        let _ = app.emit("trigger-update-check", ());
                    } else if id == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            // 앱 시작 시 백그라운드에서 GitHub 최신 릴리즈 자동 체크
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Ok(info) = check_for_updates() {
                    if info.update_available {
                        let _ = handle.emit("update-available", info);
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 빨간 닫기(X) 버튼을 눌러도 앱이 종료되지 않고 창만 숨긴 채 상단 메뉴바(⚡)에 상주하도록 처리
                api.prevent_close();
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                let _ = window
                    .app_handle()
                    .set_activation_policy(ActivationPolicy::Accessory);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            create_profile,
            update_profile,
            delete_profile,
            toggle_profile,
            set_switch_mode,
            grant_permission,
            panic_restore,
            validate_hosts_text,
            check_update,
            install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SnapHost application");
}
