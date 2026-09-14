#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Local;
use codex_switch_poc::{app as proxy_app, Binding, BindingMode, Config, Profile, ProxyState};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Reverse,
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process,
    sync::Mutex,
    time::{Duration, SystemTime},
};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent, Wry,
};
use tokio::task::JoinHandle;

const CODEX_CONFIG_BACKUP_PREFIX: &str = "config.toml.codex-switch-backup";
const MAX_SESSION_TAGS: usize = 20;
const MAX_SESSION_TAG_LENGTH: usize = 40;

struct RuntimeState {
    config_path: PathBuf,
    proxy: Mutex<Option<ProxyHandle>>,
    tray_status: Mutex<Option<MenuItem<Wry>>>,
    tray_proxy: Mutex<Option<MenuItem<Wry>>>,
    tray_takeover: Mutex<Option<MenuItem<Wry>>>,
}

struct ProxyHandle {
    listen: String,
    state: ProxyState,
    task: JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
struct Dashboard {
    proxy_running: bool,
    listen: Option<String>,
    codex_proxy_enabled: bool,
    default_profile: String,
    profiles: Vec<ProfileSummary>,
    sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct ProfileSummary {
    id: String,
    base_url: String,
    is_default: bool,
    auth_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SessionSummary {
    id: String,
    title: String,
    project_dir: String,
    created_at: u64,
    last_active_at: u64,
    size_bytes: u64,
    binding_mode: String,
    profile_id: Option<String>,
    tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileInput {
    id: String,
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
}

fn main() {
    let app = tauri::Builder::default()
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    #[cfg(target_os = "macos")]
                    {
                        let app = window.app_handle();
                        let _ = app.set_dock_visibility(false);
                        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                    }
                }
            }
        })
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|error| format!("resolve app config directory: {error}"))?;
            fs::create_dir_all(&config_dir)
                .map_err(|error| format!("create app config directory: {error}"))?;
            let config_path = config_dir.join("config.json");
            ensure_config(&config_path)?;
            if let Err(error) = recover_stale_codex_takeover() {
                eprintln!("恢复残留 Codex 接管失败: {error}");
            }
            app.manage(RuntimeState {
                config_path,
                proxy: Mutex::new(None),
                tray_status: Mutex::new(None),
                tray_proxy: Mutex::new(None),
                tray_takeover: Mutex::new(None),
            });
            let show = MenuItemBuilder::with_id("show", "显示 Codex Switch").build(app)?;
            let status = MenuItemBuilder::with_id("status", "状态：Proxy 已停止 · Codex 未接管")
                .enabled(false)
                .build(app)?;
            let toggle_proxy = MenuItemBuilder::with_id("toggle_proxy", "启动 Proxy").build(app)?;
            let toggle_takeover =
                MenuItemBuilder::with_id("toggle_takeover", "接管 Codex").build(app)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let separator_after_actions = PredefinedMenuItem::separator(app)?;
            let quit = MenuItemBuilder::with_id("quit", "退出 Codex Switch").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[
                    &show,
                    &status,
                    &separator,
                    &toggle_proxy,
                    &toggle_takeover,
                    &separator_after_actions,
                    &quit,
                ])
                .build()?;
            let menu_icon = Image::new(include_bytes!("../icons/menu-icon.rgba"), 44, 44);
            TrayIconBuilder::new()
                .icon(menu_icon)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|app, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show_main_window(app.app_handle());
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        show_main_window(app);
                    }
                    "toggle_proxy" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<RuntimeState>();
                            let running = proxy_is_running(&state);
                            let result = if running {
                                stop_proxy_impl(&state).await
                            } else {
                                start_proxy_impl(&state).await
                            };
                            let message = match result {
                                Ok(()) => {
                                    if running {
                                        "Proxy 已停止".to_string()
                                    } else {
                                        "Proxy 已启动".to_string()
                                    }
                                }
                                Err(error) => {
                                    eprintln!("托盘切换 Proxy 失败: {error}");
                                    error
                                }
                            };
                            show_main_window(&app);
                            notify_state_changed(&app, Some(&message));
                        });
                    }
                    "toggle_takeover" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<RuntimeState>();
                            let takeover_enabled = codex_home()
                                .ok()
                                .is_some_and(|root| codex_takeover_enabled(&root));
                            let result = if takeover_enabled {
                                restore_codex_files_from_home()
                            } else {
                                takeover_codex_impl(&state).await
                            };
                            let message = match result {
                                Ok(()) => {
                                    if takeover_enabled {
                                        "已取消接管，Codex 已回到原配置".to_string()
                                    } else {
                                        "Codex 已接入本地 Proxy".to_string()
                                    }
                                }
                                Err(error) => {
                                    eprintln!("托盘切换 Codex 接管失败: {error}");
                                    error
                                }
                            };
                            show_main_window(&app);
                            notify_state_changed(&app, Some(&message));
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let state = app.state::<RuntimeState>();
            state
                .tray_status
                .lock()
                .map_err(|_| "tray status lock poisoned".to_string())?
                .replace(status);
            state
                .tray_proxy
                .lock()
                .map_err(|_| "tray proxy lock poisoned".to_string())?
                .replace(toggle_proxy);
            state
                .tray_takeover
                .lock()
                .map_err(|_| "tray takeover lock poisoned".to_string())?
                .replace(toggle_takeover);
            update_tray_menu(app.app_handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            start_proxy,
            stop_proxy,
            takeover_codex,
            restore_codex,
            save_profile,
            delete_profile,
            set_default_profile,
            set_session_binding,
            set_session_tags,
            import_existing_profiles,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Codex Switch");
    app.run(|_app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            if let Err(error) = restore_codex_files_from_home() {
                eprintln!("退出前取消接管 Codex 配置失败: {error}");
            }
        }
    });
}

#[tauri::command]
async fn get_dashboard(state: State<'_, RuntimeState>) -> Result<Dashboard, String> {
    let config = read_config(&state.config_path)?;
    let proxy_running = state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?
        .is_some();
    let listen = state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?
        .as_ref()
        .map(|proxy| proxy.listen.clone());
    let codex_proxy_enabled = codex_home()
        .ok()
        .is_some_and(|root| codex_takeover_enabled(&root));
    let bindings = config.bindings.clone();
    let session_tags = config.session_tags.clone();
    let sessions = tokio::task::spawn_blocking(scan_codex_sessions)
        .await
        .map_err(|error| format!("scan sessions: {error}"))?;
    let profiles = config
        .profiles
        .iter()
        .map(|(id, profile)| ProfileSummary {
            id: id.clone(),
            base_url: profile.base_url.clone(),
            is_default: id == &config.default_profile,
            auth_configured: profile.headers.keys().any(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "authorization" | "x-api-key" | "chatgpt-account-id"
                )
            }),
        })
        .collect();
    let sessions = sessions
        .into_iter()
        .map(|session| {
            let (binding_mode, profile_id) = match bindings.get(&session.id) {
                Some(binding) if binding.mode == BindingMode::Fixed => {
                    ("fixed".to_string(), binding.profile.clone())
                }
                Some(_) => ("global".to_string(), None),
                None => ("unbound".to_string(), None),
            };
            let tags = session_tags.get(&session.id).cloned().unwrap_or_default();
            SessionSummary {
                id: session.id,
                title: session.title,
                project_dir: session.project_dir,
                created_at: session.created_at,
                last_active_at: session.last_active_at,
                size_bytes: session.size_bytes,
                binding_mode,
                profile_id,
                tags,
            }
        })
        .collect();

    Ok(Dashboard {
        proxy_running,
        listen,
        codex_proxy_enabled,
        default_profile: config.default_profile,
        profiles,
        sessions,
    })
}

#[tauri::command]
async fn start_proxy(app: AppHandle<Wry>, state: State<'_, RuntimeState>) -> Result<(), String> {
    let result = start_proxy_impl(&state).await;
    notify_state_changed(&app, None);
    result
}

async fn start_proxy_impl(state: &RuntimeState) -> Result<(), String> {
    if state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?
        .is_some()
    {
        return Ok(());
    }

    let config = read_config(&state.config_path)?;
    let listen = config.listen.clone();
    let proxy_state = ProxyState::new(config)?;
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|error| format!("bind local proxy {listen}: {error}"))?;
    let actual_listen = listener
        .local_addr()
        .map_err(|error| format!("read local proxy address: {error}"))?
        .to_string();
    let proxy_task_state = proxy_state.clone();
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, proxy_app(proxy_task_state)).await {
            eprintln!("local proxy stopped: {error}");
        }
    });
    let mut proxy = state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?;
    if proxy.is_some() {
        task.abort();
        return Ok(());
    }
    *proxy = Some(ProxyHandle {
        listen: actual_listen,
        state: proxy_state,
        task,
    });
    Ok(())
}

#[tauri::command]
async fn stop_proxy(app: AppHandle<Wry>, state: State<'_, RuntimeState>) -> Result<(), String> {
    let result = stop_proxy_impl(&state).await;
    notify_state_changed(&app, None);
    result
}

async fn stop_proxy_impl(state: &RuntimeState) -> Result<(), String> {
    if codex_home()
        .ok()
        .is_some_and(|root| codex_takeover_enabled(&root))
    {
        restore_codex_files_from_home()?;
    }
    let handle = state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?
        .take();
    if let Some(handle) = handle {
        handle.task.abort();
    }
    Ok(())
}

#[tauri::command]
async fn takeover_codex(app: AppHandle<Wry>, state: State<'_, RuntimeState>) -> Result<(), String> {
    let result = takeover_codex_impl(&state).await;
    notify_state_changed(&app, None);
    result
}

async fn takeover_codex_impl(state: &RuntimeState) -> Result<(), String> {
    let listen = state
        .proxy
        .lock()
        .map_err(|_| "proxy state lock poisoned".to_string())?
        .as_ref()
        .map(|proxy| proxy.listen.clone())
        .ok_or_else(|| "请先启动 Proxy".to_string())?;
    let root = codex_home()?;
    let config_path = root.join("config.toml");
    let marker_path = root.join("config.toml.codex-switch-session");
    let text = fs::read_to_string(&config_path)
        .map_err(|error| format!("读取 Codex 配置 {}: {error}", config_path.display()))?;
    let proxy_url = format!("http://{listen}");
    if provider_base_url(&text, "OpenAI").as_deref() == Some(proxy_url.as_str()) {
        fs::write(&marker_path, process::id().to_string())
            .map_err(|error| format!("写入 Codex 接管状态失败: {error}"))?;
        return Ok(());
    }
    let backup_path = next_codex_backup_path(&root);
    fs::copy(&config_path, &backup_path)
        .map_err(|error| format!("创建 Codex 配置备份失败: {error}"))?;
    copy_permissions(&config_path, &backup_path)?;
    let replaced = replace_provider_base_url(&text, "OpenAI", &proxy_url)?;
    if let Err(error) = write_text_atomic(&config_path, &replaced) {
        if backup_path.exists() {
            let _ = fs::remove_file(&backup_path);
        }
        return Err(error);
    }
    fs::write(&marker_path, process::id().to_string())
        .map_err(|error| format!("写入 Codex 接管状态失败: {error}"))?;
    Ok(())
}

#[tauri::command]
async fn restore_codex(app: AppHandle<Wry>, _state: State<'_, RuntimeState>) -> Result<(), String> {
    let result = restore_codex_files_from_home();
    notify_state_changed(&app, None);
    result
}

fn restore_codex_files_from_home() -> Result<(), String> {
    restore_codex_files(&codex_home()?)
}

fn restore_codex_files(root: &Path) -> Result<(), String> {
    let config_path = root.join("config.toml");
    let marker_path = root.join("config.toml.codex-switch-session");
    let Some(backup_path) = latest_codex_backup_path(root) else {
        let _ = fs::remove_file(marker_path);
        let current = fs::read_to_string(&config_path)
            .map_err(|error| format!("读取 Codex 配置 {}: {error}", config_path.display()))?;
        if provider_base_url(&current, "OpenAI")
            .is_some_and(|url| url.starts_with("http://127.0.0.1:"))
        {
            return Err("没有可用备份，无法取消接管".to_string());
        }
        return Ok(());
    };
    let current = fs::read_to_string(&config_path)
        .map_err(|error| format!("读取 Codex 配置 {}: {error}", config_path.display()))?;
    if !matches!(
        provider_base_url(&current, "OpenAI"),
        Some(url) if url.starts_with("http://127.0.0.1:")
    ) {
        let _ = fs::remove_file(marker_path);
        return Ok(());
    }
    let backup = fs::read_to_string(&backup_path)
        .map_err(|error| format!("读取 Codex 配置备份失败: {error}"))?;
    write_text_atomic(&config_path, &backup)?;
    let _ = fs::remove_file(marker_path);
    Ok(())
}

#[tauri::command]
async fn save_profile(state: State<'_, RuntimeState>, profile: ProfileInput) -> Result<(), String> {
    let id = profile.id.trim().to_string();
    let base_url = profile.base_url.trim().trim_end_matches('/').to_string();
    if id.is_empty() {
        return Err("配置档名称不能为空".to_string());
    }
    if base_url.is_empty() {
        return Err("上游地址不能为空".to_string());
    }
    let mut config = read_config(&state.config_path)?;
    let mut headers = config
        .profiles
        .get(&id)
        .map(|profile| profile.headers.clone())
        .unwrap_or_default();
    if let Some(api_key) = profile
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        headers.insert("authorization".to_string(), format!("Bearer {api_key}"));
    }
    config.profiles.insert(id, Profile { base_url, headers });
    config.validate()?;
    persist_config(&state, &config).await
}

#[tauri::command]
async fn delete_profile(state: State<'_, RuntimeState>, profile_id: String) -> Result<(), String> {
    let mut config = read_config(&state.config_path)?;
    if config.profiles.len() <= 1 {
        return Err("至少保留一个配置档，不能删除最后一个配置档".to_string());
    }
    if config.profiles.remove(&profile_id).is_none() {
        return Err(format!("配置档 {profile_id:?} 不存在"));
    }
    if profile_id == config.default_profile {
        config.default_profile = config
            .profiles
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| "删除后没有可用的全局默认配置档".to_string())?;
    }
    config.bindings.retain(|_, binding| {
        binding.mode != BindingMode::Fixed || binding.profile.as_deref() != Some(&profile_id)
    });
    config.validate()?;
    persist_config(&state, &config).await
}

#[tauri::command]
async fn set_default_profile(
    state: State<'_, RuntimeState>,
    profile_id: String,
) -> Result<(), String> {
    let mut config = read_config(&state.config_path)?;
    if !config.profiles.contains_key(&profile_id) {
        return Err(format!("配置档 {profile_id:?} 不存在"));
    }
    config.default_profile = profile_id;
    config.validate()?;
    persist_config(&state, &config).await
}

#[tauri::command]
async fn set_session_binding(
    state: State<'_, RuntimeState>,
    session_id: String,
    mode: String,
    profile_id: Option<String>,
) -> Result<(), String> {
    let mut config = read_config(&state.config_path)?;
    let mode = match mode.as_str() {
        "global" => BindingMode::Global,
        "fixed" => BindingMode::Fixed,
        _ => return Err("未知的 Session 绑定模式".to_string()),
    };
    if mode == BindingMode::Fixed {
        let profile_id = profile_id.ok_or_else(|| "固定模式需要配置档".to_string())?;
        if !config.profiles.contains_key(&profile_id) {
            return Err(format!("配置档 {profile_id:?} 不存在"));
        }
        config.bindings.insert(
            session_id,
            Binding {
                mode,
                profile: Some(profile_id),
            },
        );
    } else {
        config.bindings.insert(
            session_id,
            Binding {
                mode,
                profile: None,
            },
        );
    }
    config.validate()?;
    persist_config(&state, &config).await
}

#[tauri::command]
async fn set_session_tags(
    state: State<'_, RuntimeState>,
    session_id: String,
    tags: Vec<String>,
) -> Result<(), String> {
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("Session ID 不能为空".to_string());
    }
    let tags = normalize_session_tags(tags)?;
    let mut config = read_config(&state.config_path)?;
    if tags.is_empty() {
        config.session_tags.remove(&session_id);
    } else {
        config.session_tags.insert(session_id, tags);
    }
    config.validate()?;
    persist_config(&state, &config).await
}

#[tauri::command]
async fn import_existing_profiles(state: State<'_, RuntimeState>) -> Result<usize, String> {
    let root = codex_home()?;
    let profiles_dir = root.join("config.profiles");
    let mut entries = fs::read_dir(&profiles_dir)
        .map_err(|error| format!("读取配置档目录 {}: {error}", profiles_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    entries.sort();

    let mut config = read_config(&state.config_path)?;
    let mut imported = 0;
    for path in entries {
        let profile_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("无法识别配置档文件 {}", path.display()))?
            .to_string();
        let base_url = parse_profile_base_url(&path)?;
        let headers = read_auth_headers(
            &root
                .join("auth.profiles")
                .join(format!("{profile_id}.json")),
        )?;
        config
            .profiles
            .insert(profile_id, Profile { base_url, headers });
        imported += 1;
    }
    if imported == 0 {
        return Err(format!("未找到可导入的配置档：{}", profiles_dir.display()));
    }
    if !config.profiles.contains_key(&config.default_profile) {
        config.default_profile = if config.profiles.contains_key("sakura") {
            "sakura".to_string()
        } else {
            config
                .profiles
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| "导入后没有可用配置档".to_string())?
        };
    }
    config.validate()?;
    persist_config(&state, &config).await?;
    Ok(imported)
}

async fn persist_config(state: &RuntimeState, config: &Config) -> Result<(), String> {
    write_config(&state.config_path, config)?;
    let proxy_state = {
        let proxy = state
            .proxy
            .lock()
            .map_err(|_| "proxy state lock poisoned".to_string())?;
        proxy.as_ref().map(|handle| handle.state.clone())
    };
    if let Some(proxy_state) = proxy_state {
        proxy_state.replace_config(config.clone()).await?;
    }
    Ok(())
}

fn codex_takeover_enabled(root: &Path) -> bool {
    let config_path = root.join("config.toml");
    fs::read_to_string(config_path)
        .ok()
        .and_then(|text| provider_base_url(&text, "OpenAI"))
        .is_some_and(|url| url.starts_with("http://127.0.0.1:"))
}

fn recover_stale_codex_takeover() -> Result<(), String> {
    let root = codex_home()?;
    let marker_path = root.join("config.toml.codex-switch-session");
    if !codex_takeover_enabled(&root) {
        let _ = fs::remove_file(marker_path);
        return Ok(());
    }

    let active_pid = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if active_pid.is_some_and(process_is_alive) {
        return Ok(());
    }

    restore_codex_files(&root)
}

fn next_codex_backup_path(root: &Path) -> PathBuf {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    next_codex_backup_path_for_timestamp(root, &timestamp.to_string())
}

fn next_codex_backup_path_for_timestamp(root: &Path, timestamp: &str) -> PathBuf {
    let base = format!("{CODEX_CONFIG_BACKUP_PREFIX}-{timestamp}");
    let path = root.join(&base);
    if !path.exists() {
        return path;
    }
    for index in 2.. {
        let path = root.join(format!("{base}-{index}"));
        if !path.exists() {
            return path;
        }
    }
    unreachable!("unbounded suffix search should return a backup path")
}

fn latest_codex_backup_path(root: &Path) -> Option<PathBuf> {
    let mut backups = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == CODEX_CONFIG_BACKUP_PREFIX
                        || name.starts_with(&format!("{CODEX_CONFIG_BACKUP_PREFIX}-"))
                })
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups.pop()
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

fn show_main_window(app: &AppHandle<Wry>) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app.set_dock_visibility(true);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn proxy_is_running(state: &RuntimeState) -> bool {
    state
        .proxy
        .lock()
        .map(|proxy| proxy.is_some())
        .unwrap_or(false)
}

fn notify_state_changed(app: &AppHandle<Wry>, message: Option<&str>) {
    update_tray_menu(app);
    let _ = app.emit("codex-switch-state-changed", message);
}

fn update_tray_menu(app: &AppHandle<Wry>) {
    let state = app.state::<RuntimeState>();
    let proxy_running = proxy_is_running(&state);
    let takeover_enabled = codex_home()
        .ok()
        .is_some_and(|root| codex_takeover_enabled(&root));
    let proxy_label = if proxy_running {
        "停止 Proxy"
    } else {
        "启动 Proxy"
    };
    let takeover_label = if takeover_enabled {
        "取消接管"
    } else {
        "接管 Codex"
    };
    let status_label = format!(
        "状态：Proxy {} · Codex {}",
        if proxy_running {
            "运行中"
        } else {
            "已停止"
        },
        if takeover_enabled {
            "已接管"
        } else {
            "未接管"
        }
    );

    if let Ok(item) = state.tray_status.lock() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(status_label);
        }
    }
    if let Ok(item) = state.tray_proxy.lock() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(proxy_label);
        }
    }
    if let Ok(item) = state.tray_takeover.lock() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(takeover_label);
            let _ = item.set_enabled(proxy_running || takeover_enabled);
        }
    };
}

fn provider_base_url(text: &str, provider: &str) -> Option<String> {
    let section = format!("[model_providers.{provider}]");
    let mut in_provider = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_provider = trimmed == section;
        }
        if !in_provider {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "base_url" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn replace_provider_base_url(text: &str, provider: &str, base_url: &str) -> Result<String, String> {
    let section = format!("[model_providers.{provider}]");
    let mut in_provider = false;
    let mut replaced = false;
    let mut output = String::with_capacity(text.len() + base_url.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_provider = trimmed == section;
        }
        if in_provider {
            if let Some((key, _)) = trimmed.split_once('=') {
                if key.trim() == "base_url" {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    output.push_str(indent);
                    output.push_str("base_url = \"");
                    output.push_str(base_url);
                    output.push_str("\"\n");
                    replaced = true;
                    continue;
                }
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    if !replaced {
        return Err(format!(
            "Codex 配置中未找到 [model_providers.{provider}] 的 base_url"
        ));
    }
    if !text.ends_with('\n') {
        output.pop();
    }
    Ok(output)
}

fn copy_permissions(source: &Path, target: &Path) -> Result<(), String> {
    let permissions = fs::metadata(source)
        .map_err(|error| format!("读取文件权限失败: {error}"))?
        .permissions();
    fs::set_permissions(target, permissions)
        .map_err(|error| format!("设置备份文件权限失败: {error}"))
}

fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    let temp_path = path.with_extension("codex-switch.tmp");
    fs::write(&temp_path, text)
        .map_err(|error| format!("写入临时配置 {} 失败: {error}", temp_path.display()))?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temp_path, metadata.permissions())
            .map_err(|error| format!("设置配置权限失败: {error}"))?;
    }
    fs::rename(&temp_path, path)
        .map_err(|error| format!("替换配置 {} 失败: {error}", path.display()))
}

fn codex_home() -> Result<PathBuf, String> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| "无法确定 CODEX_HOME 或 HOME".to_string())
}

fn parse_profile_base_url(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取配置档 {}: {error}", path.display()))?;
    let document = text
        .parse::<toml::Value>()
        .map_err(|error| format!("解析配置档 {}: {error}", path.display()))?;
    let providers = document
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("配置档 {} 缺少 model_providers", path.display()))?;
    let provider = providers
        .get("OpenAI")
        .or_else(|| providers.values().next())
        .ok_or_else(|| format!("配置档 {} 没有 provider", path.display()))?;
    let base_url = provider
        .get("base_url")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| format!("配置档 {} 缺少 base_url", path.display()))?;
    Ok(base_url.trim_end_matches('/').to_string())
}

fn read_auth_headers(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取认证文件 {}: {error}", path.display()))?;
    let value = serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("解析认证文件 {}: {error}", path.display()))?;
    let mut headers = BTreeMap::new();
    if let Some(api_key) = value
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        headers.insert("authorization".to_string(), format!("Bearer {api_key}"));
        return Ok(headers);
    }
    let Some(tokens) = value.get("tokens").and_then(Value::as_object) else {
        return Ok(headers);
    };
    if let Some(access_token) = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {access_token}"),
        );
    }
    if let Some(account_id) = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|account| !account.is_empty())
    {
        headers.insert("chatgpt-account-id".to_string(), account_id.to_string());
    }
    Ok(headers)
}

fn ensure_config(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let config = Config {
        listen: "127.0.0.1:8787".to_string(),
        default_profile: "sakura".to_string(),
        profiles: BTreeMap::from([(
            "sakura".to_string(),
            Profile {
                base_url: "https://api.openai.com/v1".to_string(),
                headers: BTreeMap::new(),
            },
        )]),
        bindings: BTreeMap::new(),
        session_tags: BTreeMap::new(),
    };
    write_config(path, &config)
}

fn read_config(path: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read config {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("parse config {}: {error}", path.display()))
}

fn normalize_session_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_SESSION_TAGS {
        return Err(format!("每个 Session 最多设置 {MAX_SESSION_TAGS} 个标签"));
    }
    let mut normalized = Vec::new();
    for tag in tags {
        let tag = tag.split_whitespace().collect::<Vec<_>>().join(" ");
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_SESSION_TAG_LENGTH {
            return Err(format!("单个标签不能超过 {MAX_SESSION_TAG_LENGTH} 个字符"));
        }
        if normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&tag))
        {
            continue;
        }
        normalized.push(tag);
    }
    Ok(normalized)
}

fn write_config(path: &Path, config: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| format!("serialize config: {error}"))?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, format!("{text}\n"))
        .map_err(|error| format!("write config {}: {error}", temp_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("set config permissions: {error}"))?;
    }
    fs::rename(&temp_path, path)
        .map_err(|error| format!("replace config {}: {error}", path.display()))
}

#[derive(Default)]
struct ScannedSession {
    id: String,
    title: String,
    project_dir: String,
    created_at: u64,
    last_active_at: u64,
    size_bytes: u64,
}

#[derive(Default)]
struct StoredSession {
    title: String,
    project_dir: String,
    created_at: u64,
    last_active_at: u64,
    is_subagent: bool,
}

fn scan_codex_sessions() -> Vec<ScannedSession> {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    let Some(root) = root else {
        return Vec::new();
    };
    let thread_titles = load_thread_titles(&root.join("session_index.jsonl"));
    let stored_sessions = load_state_sessions(&codex_state_db_paths(&root));
    let mut files = Vec::new();
    for folder in [root.join("sessions"), root.join("archived_sessions")] {
        collect_jsonl_files(&folder, &mut files);
    }
    let mut sessions = files
        .into_iter()
        .filter_map(|path| parse_session_file(&path, &thread_titles, &stored_sessions))
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| Reverse(session.last_active_at));
    sessions
}

fn codex_state_db_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.join("state_5.sqlite")];
    let configured_home = fs::read_to_string(root.join("config.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
        .and_then(|value| {
            value
                .get("sqlite_home")
                .and_then(toml::Value::as_str)
                .map(PathBuf::from)
        });
    let env_home = std::env::var_os("CODEX_SQLITE_HOME").map(PathBuf::from);
    if let Some(home) = configured_home.or(env_home) {
        let home = if home == Path::new("~") {
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or(home)
        } else if let Ok(rest) = home.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(rest))
                .unwrap_or(home)
        } else {
            home
        };
        let path = home.join("state_5.sqlite");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn load_state_sessions(paths: &[PathBuf]) -> BTreeMap<String, StoredSession> {
    let mut sessions = BTreeMap::new();
    for path in paths {
        let Ok(connection) = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            continue;
        };
        let _ = connection.busy_timeout(Duration::from_secs(1));
        let Ok(mut statement) = connection.prepare(
            "SELECT id, title, created_at_ms, updated_at_ms, cwd, source \
             FROM threads \
             WHERE title <> '' \
             AND (first_user_message IS NULL OR TRIM(title) <> TRIM(first_user_message))",
        ) else {
            continue;
        };
        let Ok(rows) = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let created_at = row.get::<_, Option<i64>>(2)?.unwrap_or_default().max(0) as u64;
            let last_active_at = row.get::<_, Option<i64>>(3)?.unwrap_or_default().max(0) as u64;
            let project_dir: String = row.get(4)?;
            let source: String = row.get(5)?;
            Ok((
                id,
                StoredSession {
                    title: compact_text(&title, 100),
                    project_dir,
                    created_at,
                    last_active_at,
                    is_subagent: source.to_ascii_lowercase().contains("subagent"),
                },
            ))
        }) else {
            continue;
        };
        for row in rows.flatten() {
            sessions.insert(row.0, row.1);
        }
    }
    sessions
}

fn load_thread_titles(path: &Path) -> BTreeMap<String, String> {
    let Ok(file) = File::open(path) else {
        return BTreeMap::new();
    };
    BufReader::new(file)
        .lines()
        .flatten()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .filter_map(|value| {
            let id = value.get("id")?.as_str()?.trim();
            let title = value.get("thread_name")?.as_str()?.trim();
            if id.is_empty() || title.is_empty() {
                return None;
            }
            Some((id.to_string(), compact_text(title, 100)))
        })
        .collect()
}

fn collect_jsonl_files(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(path);
        }
    }
}

fn parse_session_file(
    path: &Path,
    thread_titles: &BTreeMap<String, String>,
    stored_sessions: &BTreeMap<String, StoredSession>,
) -> Option<ScannedSession> {
    let metadata = fs::metadata(path).ok()?;
    let created_at = metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()
        .and_then(system_time_millis)
        .unwrap_or_default();
    let last_active_at = metadata
        .modified()
        .ok()
        .and_then(system_time_millis)
        .unwrap_or(created_at);
    let fallback_id = path.file_stem()?.to_string_lossy().to_string();
    let mut session = ScannedSession {
        id: fallback_id,
        created_at,
        last_active_at,
        size_bytes: metadata.len(),
        ..Default::default()
    };
    if stored_sessions
        .get(&session.id)
        .is_some_and(|stored| stored.is_subagent)
    {
        return None;
    }
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(80).flatten() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|value| value.as_str()) == Some("session_meta") {
            if value
                .get("payload")
                .and_then(|payload| payload.get("source"))
                .is_some_and(json_is_subagent_source)
            {
                return None;
            }
            if let Some(id) = value
                .get("payload")
                .and_then(|payload| payload.get("id"))
                .and_then(|value| value.as_str())
            {
                session.id = id.to_string();
            }
            if let Some(cwd) = value
                .get("payload")
                .and_then(|payload| payload.get("cwd"))
                .and_then(|value| value.as_str())
            {
                session.project_dir = cwd.to_string();
            }
        }
        if session.title.is_empty()
            && value.get("type").and_then(|value| value.as_str()) == Some("response_item")
            && value
                .get("payload")
                .and_then(|payload| payload.get("role"))
                .and_then(|value| value.as_str())
                == Some("user")
        {
            session.title = value
                .get("payload")
                .and_then(|payload| payload.get("content"))
                .map(extract_text)
                .unwrap_or_default();
            session.title = compact_text(&session.title, 100);
        }
    }
    if session.title.is_empty() {
        session.title = "未命名 Session".to_string();
    }
    if let Some(stored) = stored_sessions.get(&session.id) {
        if stored.is_subagent {
            return None;
        }
        if stored.created_at > 0 {
            session.created_at = stored.created_at;
        }
        if stored.last_active_at > 0 {
            session.last_active_at = stored.last_active_at;
        }
        if !stored.project_dir.is_empty() {
            session.project_dir = stored.project_dir.clone();
        }
        if !stored.title.is_empty() {
            session.title = stored.title.clone();
        }
    } else if let Some(title) = thread_titles.get(&session.id) {
        session.title = title.clone();
    } else if is_placeholder_title(&session.title) {
        let folder_name = Path::new(&session.project_dir)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("当前项目");
        session.title = format!("Codex Session · {folder_name}");
    }
    Some(session)
}

fn json_is_subagent_source(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(source) => source.to_ascii_lowercase().contains("subagent"),
        serde_json::Value::Object(source) => source.contains_key("subagent"),
        _ => false,
    }
}

fn is_placeholder_title(title: &str) -> bool {
    let title = title.to_ascii_lowercase();
    title.contains("agents.md instructions")
        || title.contains("<instructions>")
        || title.contains("you are codex")
        || looks_like_uuid(title.split_whitespace().next().unwrap_or_default())
}

fn looks_like_uuid(value: &str) -> bool {
    let groups = value.split('-').collect::<Vec<_>>();
    [8, 4, 4, 4, 12].iter().enumerate().all(|(index, length)| {
        groups.get(index).is_some_and(|group| {
            group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    })
}

fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(extract_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::Object(map) => map
            .get("text")
            .or_else(|| map.get("value"))
            .map(extract_text)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn system_time_millis(time: SystemTime) -> Option<u64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_base_url_replacement_preserves_other_config() {
        let text = "model = \"gpt-5\"\n\n[model_providers.OpenAI]\nname = \"OpenAI\"\nbase_url = 'https://example.com/v1'\n\n[features]\ngoals = true\n";
        let replaced = replace_provider_base_url(text, "OpenAI", "http://127.0.0.1:8787").unwrap();
        assert_eq!(
            provider_base_url(&replaced, "OpenAI").as_deref(),
            Some("http://127.0.0.1:8787")
        );
        assert!(replaced.contains("model = \"gpt-5\""));
        assert!(replaced.contains("[features]\ngoals = true"));
    }

    #[test]
    fn latest_backup_prefers_dated_backup_and_keeps_legacy_compatible() {
        let root = std::env::temp_dir().join(format!("codex-switch-backup-test-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(CODEX_CONFIG_BACKUP_PREFIX), "legacy").unwrap();
        fs::write(
            root.join(format!("{CODEX_CONFIG_BACKUP_PREFIX}-20260909-130001")),
            "new",
        )
        .unwrap();

        assert_eq!(
            latest_codex_backup_path(&root)
                .unwrap()
                .file_name()
                .unwrap()
                .to_str(),
            Some("config.toml.codex-switch-backup-20260909-130001")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backup_path_adds_suffix_when_timestamp_exists() {
        let root =
            std::env::temp_dir().join(format!("codex-switch-backup-collision-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(format!("{CODEX_CONFIG_BACKUP_PREFIX}-20260909-130001")),
            "taken",
        )
        .unwrap();

        assert_eq!(
            next_codex_backup_path_for_timestamp(&root, "20260909-130001")
                .file_name()
                .unwrap()
                .to_str(),
            Some("config.toml.codex-switch-backup-20260909-130001-2")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subagent_sources_are_filtered() {
        assert!(json_is_subagent_source(&serde_json::json!({
            "subagent": {"thread_spawn": {}}
        })));
        assert!(json_is_subagent_source(&serde_json::json!("subagent")));
        assert!(!json_is_subagent_source(&serde_json::json!("cli")));
    }

    #[test]
    fn session_tags_are_normalized_for_storage() {
        assert_eq!(
            normalize_session_tags(vec![
                "  her  ".to_string(),
                "HER".to_string(),
                "待   复盘".to_string(),
                String::new(),
            ])
            .unwrap(),
            vec!["her".to_string(), "待 复盘".to_string()]
        );
    }
}
