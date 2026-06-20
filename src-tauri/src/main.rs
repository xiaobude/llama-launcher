#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic_proxy;

use std::fs;
use std::net::TcpListener;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use serde_json::Value;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

struct ServerPid(Mutex<Option<u32>>);
struct ProxyHandle(Mutex<Option<tokio::task::JoinHandle<()>>>);

fn get_log_dir(_app: &tauri::AppHandle) -> PathBuf {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("logs")))
        .unwrap_or_else(|| PathBuf::from("logs"));
    let _ = fs::create_dir_all(&path);
    path
}

fn kill_process(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

fn is_port_in_use(port: u16) -> bool {
    TcpListener::bind(format!("127.0.0.1:{}", port)).is_err()
}

fn wait_for_port_free(port: u16, max_wait: u64) {
    let start = Instant::now();
    while is_port_in_use(port) {
        if start.elapsed() > Duration::from_secs(max_wait) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn rotate_log(path: &PathBuf, max_size: u64) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > max_size => {
            let mut i = 5;
            while i > 0 {
                let old = format!("{}.{}", path.to_string_lossy(), i - 1);
                let new = format!("{}.{}", path.to_string_lossy(), i);
                if PathBuf::from(&old).exists() {
                    let _ = fs::rename(&old, &new);
                }
                i -= 1;
            }
            if path.exists() {
                let _ = fs::rename(path, format!("{}.1", path.to_string_lossy()));
            }
        }
        _ => {}
    }
}

#[tauri::command]
async fn start_server(
    state: State<'_, ServerPid>,
    proxy_state: State<'_, ProxyHandle>,
    app: tauri::AppHandle,
    args: Vec<String>,
    cuda_device: String,
    enable_anthropic_proxy: bool,
    anthropic_proxy_port: u16,
    anthropic_api_key: String,
) -> Result<u32, String> {
    if args.is_empty() || args[0].is_empty() {
        return Err("服务器路径不能为空".to_string());
    }
    if !std::path::Path::new(&args[0]).exists() {
        return Err(format!("服务器程序不存在: {}", args[0]));
    }

    let port = args
        .iter()
        .enumerate()
        .find_map(|(i, a)| {
            if a == "--port" || a == "-p" {
                args.get(i + 1).and_then(|p| p.parse::<u16>().ok())
            } else {
                None
            }
        })
        .unwrap_or(8080);

    {
        let mut guard = state.0.lock().unwrap();
        if let Some(pid) = *guard {
            kill_process(pid);
            *guard = None;
        }
    }

    wait_for_port_free(port, 5);

    if is_port_in_use(port) {
        return Err(format!("端口 {} 仍被占用，无法启动服务（旧服务已停止）", port));
    }

    let log_dir = get_log_dir(&app);
    let log_file = log_dir.join("llama-server.log");
    rotate_log(&log_file, 10 * 1024 * 1024);

    let log_out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file)
        .map_err(|e| format!("无法创建日志文件: {}", e))?;
    let log_err = log_out.try_clone().map_err(|e| e.to_string())?;

    let filtered_args: Vec<&str> = args[1..]
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| a.as_str())
        .collect();

    let mut cmd = Command::new(&args[0]);
    cmd.args(&filtered_args)
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err))
        .creation_flags(CREATE_NO_WINDOW);

    if !cuda_device.is_empty() {
        cmd.env("CUDA_VISIBLE_DEVICES", &cuda_device);
    }

    let child = cmd.spawn().map_err(|e| format!("无法启动进程: {}", e))?;
    let pid = child.id();

    {
        let mut guard = state.0.lock().unwrap();
        *guard = Some(pid);
    }

    let _ = fs::write(log_dir.join("server.pid"), pid.to_string());

    {
        let mut proxy_guard = proxy_state.0.lock().unwrap();
        if let Some(old) = proxy_guard.take() {
            old.abort();
        }
        if enable_anthropic_proxy {
            let handle = tokio::spawn(anthropic_proxy::run_proxy(
                anthropic_proxy_port,
                port,
                anthropic_api_key,
            ));
            *proxy_guard = Some(handle);
        }
    }

    Ok(pid)
}

#[tauri::command]
async fn stop_server(
    state: State<'_, ServerPid>,
    proxy_state: State<'_, ProxyHandle>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(pid) = *guard {
        kill_process(pid);
        *guard = None;
    }
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "llama-server.exe"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    if let Some(handle) = proxy_state.0.lock().unwrap().take() {
        handle.abort();
    }

    std::thread::sleep(Duration::from_millis(500));

    let _ = fs::remove_file(get_log_dir(&app).join("server.pid"));

    Ok("已停止".to_string())
}

// Looks beside the exe (and up to 5 parent dirs, plus a bundled "resources"
// subfolder at each level) so config.json/profiles.json can be hand-placed
// next to the install, copied elsewhere, or shipped via the installer.
fn find_file_path(app: &tauri::AppHandle, filename: &str) -> PathBuf {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let mut current = exe_dir.to_path_buf();
            for _ in 0..6 {
                let direct = current.join(filename);
                if direct.exists() {
                    return direct;
                }
                let nested = current.join("resources").join(filename);
                if nested.exists() {
                    return nested;
                }
                match current.parent() {
                    Some(parent) => current = parent.to_path_buf(),
                    None => break,
                }
            }
        }
    }

    if let Ok(p) = app
        .path()
        .resolve(format!("resources/{}", filename), tauri::path::BaseDirectory::Resource)
    {
        if p.exists() {
            return p;
        }
    }

    PathBuf::from(filename)
}

fn parse_json_file(path: &PathBuf) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{FEFF}')).ok())
}

#[tauri::command]
async fn load_builtins(app: tauri::AppHandle) -> Result<Value, String> {
    let path = find_file_path(&app, "config.json");
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    parse_json_file(&path).ok_or_else(|| "内置配置文件解析失败".to_string())
}

#[tauri::command]
async fn load_profiles(app: tauri::AppHandle) -> Result<Value, String> {
    let path = find_file_path(&app, "profiles.json");
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    parse_json_file(&path).ok_or_else(|| "自定义配置文件解析失败".to_string())
}

#[tauri::command]
async fn save_profiles(app: tauri::AppHandle, profiles: Value) -> Result<String, String> {
    let path = find_file_path(&app, "profiles.json");
    let target_path = if path.exists() {
        path
    } else if let Ok(exe_path) = std::env::current_exe() {
        exe_path
            .parent()
            .map(|d| d.join("profiles.json"))
            .unwrap_or_else(|| PathBuf::from("profiles.json"))
    } else {
        PathBuf::from("profiles.json")
    };
    let json = serde_json::to_string_pretty(&profiles).map_err(|e| e.to_string())?;
    fs::write(&target_path, json).map_err(|e| e.to_string())?;
    Ok("已保存".to_string())
}

#[tauri::command]
async fn open_log(app: tauri::AppHandle) -> Result<(), String> {
    let log_file = get_log_dir(&app).join("llama-server.log");
    let log_path = log_file.to_string_lossy().replace('\'', "''");

    let cmd = format!(
        "if(Test-Path '{p}'){{ Get-Content '{p}' -Encoding UTF8 -Wait -Tail 200 }} else {{ Write-Host '日志文件尚未创建，请先启动服务' -ForegroundColor Red; Start-Sleep 5 }}",
        p = log_path
    );

    Command::new("powershell")
        .args(["-NoProfile", "-Command", &cmd])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
async fn browse_file(app: tauri::AppHandle, filter_name: String, extension: String) -> Result<String, String> {
    let mut builder = app.dialog().file();
    if !extension.is_empty() {
        builder = builder.add_filter(&filter_name, &[&extension]);
    }
    let file_path = builder.blocking_pick_file();
    match file_path {
        Some(p) => Ok(p.to_string()),
        None => Ok("".to_string()),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerPid(Mutex::new(None)))
        .manage(ProxyHandle(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            start_server,
            stop_server,
            load_builtins,
            load_profiles,
            save_profiles,
            open_log,
            browse_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
