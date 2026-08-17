#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic_proxy;
mod gguf;

use gguf::{estimate_vram, inspect_gguf_file, GgufInfo, VramEstimate};

use std::fs;
use std::net::TcpListener;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const CREATE_NEW_CONSOLE: u32 = 0x00000010;

struct ServerPid(Mutex<Option<u32>>);
struct ProxyHandle(Mutex<Option<tokio::task::JoinHandle<()>>>);
struct ServerPort(Mutex<Option<u16>>);
struct CompileChild(Mutex<Option<u32>>);

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
    port_state: State<'_, ServerPort>,
    app: tauri::AppHandle,
    mut args: Vec<String>,
    cuda_device: String,
    enable_anthropic_proxy: bool,
    anthropic_proxy_port: u16,
    anthropic_api_key: String,
) -> Result<u32, String> {
    if args.is_empty() || args[0].is_empty() || !std::path::Path::new(&args[0]).exists() {
        let bundled = find_file_path(&app, "llama-server.exe");
        if bundled.exists() {
            if args.is_empty() {
                args.push(bundled.to_string_lossy().to_string());
            } else {
                args[0] = bundled.to_string_lossy().to_string();
            }
        } else if args.is_empty() || args[0].is_empty() {
            return Err("服务器路径不能为空，且未找到内置引擎 (llama-server.exe)".to_string());
        } else {
            return Err(format!("服务器程序不存在: {}", args[0]));
        }
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

    // Open the log file first, then rotate — avoids losing output between
    // rename and re-open if the file just crossed the size threshold.
    let log_out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file)
        .map_err(|e| format!("无法创建日志文件: {}", e))?;
    rotate_log(&log_file, 10 * 1024 * 1024);
    let log_err = log_out.try_clone().map_err(|e| e.to_string())?;

    let filtered_args: Vec<&str> = args[1..]
        .iter()
        .filter(|a| !a.is_empty())
        .map(|a| a.as_str())
        .collect();

    let mut cmd = Command::new(&args[0]);
    cmd.args(&filtered_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);

    if !cuda_device.is_empty() {
        cmd.env("CUDA_VISIBLE_DEVICES", &cuda_device);
    }

    let mut child = cmd.spawn().map_err(|e| format!("无法启动进程: {}", e))?;
    let pid = child.id();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    use std::io::{BufRead, BufReader, Write};

    if let Some(stdout) = stdout {
        let mut log_out_clone = log_out.try_clone().map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if !l.contains("update_slots: all slots are idle") => {
                        if writeln!(log_out_clone, "{}", l).is_err() {
                            eprintln!("[Log] Failed to write stdout line to log file");
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[Log] Failed to read stdout line: {}", e);
                        break;
                    }
                }
            }
        });
    }

    if let Some(stderr) = stderr {
        let mut log_err_clone = log_err.try_clone().map_err(|e| e.to_string())?;
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) if !l.contains("update_slots: all slots are idle") => {
                        if writeln!(log_err_clone, "{}", l).is_err() {
                            eprintln!("[Log] Failed to write stderr line to log file");
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[Log] Failed to read stderr line: {}", e);
                        break;
                    }
                }
            }
        });
    }

    {
        let mut guard = state.0.lock().unwrap();
        *guard = Some(pid);
    }

    {
        let mut guard = port_state.0.lock().unwrap();
        *guard = Some(port);
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
    port_state: State<'_, ServerPort>,
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

    {
        let mut guard = port_state.0.lock().unwrap();
        *guard = None;
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
            let direct = exe_dir.join(filename);
            if direct.exists() {
                return direct;
            }
            let config_sub = exe_dir.join("config").join(filename);
            if config_sub.exists() {
                return config_sub;
            }
            let res_sub = exe_dir.join("resources").join(filename);
            if res_sub.exists() {
                return res_sub;
            }
            let logs_parent = exe_dir.join("..").join(filename);
            if logs_parent.exists() {
                return logs_parent;
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

fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            if c == '"' {
                in_string = true;
                out.push(c);
            } else if c == '/' {
                if let Some(&next) = chars.peek() {
                    if next == '/' {
                        chars.next();
                        while let Some(&nc) = chars.peek() {
                            if nc == '\n' || nc == '\r' {
                                break;
                            }
                            chars.next();
                        }
                    } else if next == '*' {
                        chars.next();
                        while let Some(nc) = chars.next() {
                            if nc == '*' {
                                if let Some(&nc2) = chars.peek() {
                                    if nc2 == '/' {
                                        chars.next();
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        out.push(c);
                    }
                } else {
                    out.push(c);
                }
            } else {
                out.push(c);
            }
        }
    }
    out
}

fn parse_json_file(path: &PathBuf) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    let clean = content.trim_start_matches('\u{FEFF}');
    if let Ok(v) = serde_json::from_str::<Value>(clean) {
        return Some(v);
    }
    let stripped = strip_json_comments(clean);
    serde_json::from_str::<Value>(&stripped).ok()
}

fn is_valid_profile_map(val: &Value) -> bool {
    if let Some(obj) = val.as_object() {
        if obj.is_empty() {
            return false;
        }
        for (_k, v) in obj {
            if let Some(profile_obj) = v.as_object() {
                if profile_obj.contains_key("modelPath")
                    || profile_obj.contains_key("serverPath")
                    || profile_obj.contains_key("port")
                    || profile_obj.contains_key("gpuLayers")
                {
                    return true;
                }
            }
        }
    }
    false
}

#[tauri::command]
async fn load_builtins(app: tauri::AppHandle) -> Result<Value, String> {
    let candidates = [
        find_file_path(&app, "config.json"),
        find_file_path(&app, "config/config.json"),
    ];

    for path in &candidates {
        if path.exists() {
            if let Some(val) = parse_json_file(path) {
                if is_valid_profile_map(&val) {
                    return Ok(val);
                }
            }
        }
    }

    Ok(Value::Object(serde_json::Map::new()))
}

#[tauri::command]
async fn load_profiles(app: tauri::AppHandle) -> Result<Value, String> {
    let candidates = [
        find_file_path(&app, "profiles.json"),
        find_file_path(&app, "config/profiles.json"),
        find_file_path(&app, "config.json"),
        find_file_path(&app, "config/config.json"),
    ];

    for path in &candidates {
        if path.exists() {
            if let Some(val) = parse_json_file(path) {
                if is_valid_profile_map(&val) {
                    return Ok(val);
                }
            }
        }
    }

    Ok(Value::Object(serde_json::Map::new()))
}

#[tauri::command]
async fn save_profiles(app: tauri::AppHandle, profiles: Value) -> Result<String, String> {
    let candidates = [
        find_file_path(&app, "profiles.json"),
        find_file_path(&app, "config/profiles.json"),
        find_file_path(&app, "config.json"),
    ];
    
    let mut target_path = candidates.iter().find(|p| p.exists()).cloned();
    if target_path.is_none() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let cfg_dir = exe_dir.join("config");
                if cfg_dir.exists() {
                    target_path = Some(cfg_dir.join("profiles.json"));
                } else {
                    target_path = Some(exe_dir.join("profiles.json"));
                }
            }
        }
    }
    let save_path = target_path.unwrap_or_else(|| PathBuf::from("profiles.json"));
    let json = serde_json::to_string_pretty(&profiles).map_err(|e| e.to_string())?;
    fs::write(&save_path, json).map_err(|e| e.to_string())?;
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
    if extension == "dir" || filter_name == "dir" || filter_name == "文件夹" {
        let folder = app.dialog().file().blocking_pick_folder();
        match folder {
            Some(p) => Ok(p.to_string()),
            None => Ok("".to_string()),
        }
    } else {
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
}

#[tauri::command]
async fn get_bundled_server_path(app: tauri::AppHandle) -> Result<String, String> {
    let path = find_file_path(&app, "llama-server.exe");
    if path.exists() {
        Ok(path.to_string_lossy().to_string())
    } else {
        Ok("".to_string())
    }
}

#[tauri::command]
async fn inspect_gguf(path: String) -> Result<GgufInfo, String> {
    let clean_path = path.trim().trim_matches('"').trim_matches('\'');
    if clean_path.is_empty() || !std::path::Path::new(clean_path).exists() {
        return Err(format!("模型文件路径无效或不存在: {}", clean_path));
    }
    inspect_gguf_file(clean_path).map_err(|e| format!("解析 GGUF 失败: {}", e))
}

#[tauri::command]
async fn estimate_vram_budget(
    path: String,
    ctx_size: u32,
    cache_type_k: String,
    cache_type_v: String,
    gpu_vram_gb: Option<f64>,
) -> Result<VramEstimate, String> {
    let clean_path = path.trim().trim_matches('"').trim_matches('\'');
    let info = inspect_gguf_file(clean_path).map_err(|e| format!("解析 GGUF 失败: {}", e))?;
    let total_vram = gpu_vram_gb.unwrap_or(16.0);
    Ok(estimate_vram(&info, ctx_size, &cache_type_k, &cache_type_v, total_vram))
}

#[derive(Serialize)]
struct BuildEnvStatus {
    cuda_installed: bool,
    cuda_version: String,
    nvcc_path: String,
    msvc_installed: bool,
    ccache_installed: bool,
    ninja_installed: bool,
    git_installed: bool,
    cmake_installed: bool,
}

#[tauri::command]
async fn check_build_env() -> Result<BuildEnvStatus, String> {
    let cuda_128 = std::path::Path::new(r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8\bin\nvcc.exe");
    let cuda_132 = std::path::Path::new(r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2\bin\nvcc.exe");
    let cuda_133 = std::path::Path::new(r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.3\bin\nvcc.exe");
    let (cuda_installed, cuda_version, nvcc_path) = if cuda_133.exists() {
        if cuda_128.exists() {
            (true, "CUDA 13.3 与 12.8 多版本就绪".to_string(), cuda_133.to_string_lossy().to_string())
        } else {
            (true, "v13.3 就绪 (C:\\Program Files\\...\\v13.3)".to_string(), cuda_133.to_string_lossy().to_string())
        }
    } else if cuda_132.exists() {
        (true, "v13.2 就绪".to_string(), cuda_132.to_string_lossy().to_string())
    } else if cuda_128.exists() {
        (true, "v12.8 就绪 (推荐 sm_120)".to_string(), cuda_128.to_string_lossy().to_string())
    } else {
        (false, "未检测到 CUDA Toolkit".to_string(), "".to_string())
    };

    let vswhere = std::path::Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    let mut msvc_installed = if vswhere.exists() {
        Command::new(vswhere)
            .args(["-products", "*", "-latest", "-property", "installationPath"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    } else {
        Command::new("vswhere.exe")
            .args(["-products", "*", "-latest"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    };

    if !msvc_installed {
        let vs_paths = [
            r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
            r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat",
            r"C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat",
            r"C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat",
            r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat",
        ];
        for p in vs_paths {
            if std::path::Path::new(p).exists() {
                msvc_installed = true;
                break;
            }
        }
    }

    let ccache_installed = Command::new("ccache.exe")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let ninja_installed = Command::new("ninja.exe")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let git_installed = Command::new("git.exe")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let cmake_installed = Command::new("cmake.exe")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    Ok(BuildEnvStatus {
        cuda_installed,
        cuda_version,
        nvcc_path,
        msvc_installed,
        ccache_installed,
        ninja_installed,
        git_installed,
        cmake_installed,
    })
}

const EMBEDDED_BUILD_SCRIPT: &str = include_str!("../../build_llama-server.ps1");

#[tauri::command]
async fn start_compile_engine(
    app: tauri::AppHandle,
    state: State<'_, CompileChild>,
    source_dir: String,
    build_number: String,
    cuda_ver: String,
    cuda_arch: String,
    cpu_arch: String,
    threads: u32,
) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(pid) = *guard {
        kill_process(pid);
        *guard = None;
    }

    let root_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let script_name = "build_llama-server.ps1";
    let script_path = if root_dir.join(script_name).exists() {
        root_dir.join(script_name)
    } else {
        let temp_script = get_log_dir(&app).join(script_name);
        let clean_script = EMBEDDED_BUILD_SCRIPT.trim_start_matches('\u{feff}');
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(clean_script.as_bytes());
        let _ = fs::write(&temp_script, bytes);
        temp_script
    };

    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script_path.to_string_lossy(),
    ]);

    if !source_dir.trim().is_empty() {
        cmd.args(["-SourceDir", source_dir.trim()]);
    }
    if !build_number.trim().is_empty() {
        cmd.args(["-BuildNumber", build_number.trim()]);
    }
    if !cuda_ver.trim().is_empty() {
        cmd.args(["-CudaVersion", cuda_ver.trim()]);
    }
    if !cuda_arch.trim().is_empty() {
        cmd.args(["-CudaArch", cuda_arch.trim()]);
    }
    if !cpu_arch.trim().is_empty() {
        cmd.args(["-CpuArch", cpu_arch.trim()]);
    }
    if threads > 0 {
        cmd.args(["-Threads", &threads.to_string()]);
    }

    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("启动编译脚本失败: {}", e))?;
    let pid = child.id();
    *guard = Some(pid);
    drop(guard);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let app_handle = app.clone();

    std::thread::spawn(move || {
        use std::io::BufRead;

        if let Some(out) = stdout {
            let mut reader = std::io::BufReader::new(out);
            let mut buf = Vec::new();
            while let Ok(n) = reader.read_until(b'\n', &mut buf) {
                if n == 0 { break; }
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if !line.is_empty() {
                    let _ = app_handle.emit("compile-log", line);
                }
                buf.clear();
            }
        }

        if let Some(err) = stderr {
            let mut reader = std::io::BufReader::new(err);
            let mut buf = Vec::new();
            while let Ok(n) = reader.read_until(b'\n', &mut buf) {
                if n == 0 { break; }
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if !line.is_empty() {
                    let _ = app_handle.emit("compile-log", format!("[STDERR] {}", line));
                }
                buf.clear();
            }
        }

        let exit_code = match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(_) => 1,
        };

        let _ = app_handle.emit("compile-finished", serde_json::json!({
            "success": exit_code == 0,
            "code": exit_code
        }));
    });

    Ok(format!("编译任务已启动 (PID: {})", pid))
}

#[tauri::command]
async fn cancel_compile_engine(state: State<'_, CompileChild>) -> Result<String, String> {
    let mut guard = state.0.lock().unwrap();
    if let Some(pid) = *guard {
        kill_process(pid);
        *guard = None;
        Ok("已发送终止信号".to_string())
    } else {
        Ok("未在运行编译任务".to_string())
    }
}

#[tauri::command]
async fn get_latest_llama_tag() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    if let Ok(res) = client.get("https://api.github.com/repos/ggml-org/llama.cpp/releases/latest").send().await {
        if let Ok(json) = res.json::<serde_json::Value>().await {
            if let Some(tag) = json.get("tag_name").and_then(|v| v.as_str()) {
                return Ok(tag.to_string());
            }
        }
    }

    let output = Command::new("git.exe")
        .args(["ls-remote", "--tags", "--refs", "https://github.com/ggml-org/llama.cpp.git"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut parsed_tags: Vec<(u32, String)> = stdout
            .lines()
            .filter_map(|l| l.split("refs/tags/").nth(1))
            .filter_map(|t| {
                if t.starts_with('b') {
                    t[1..].parse::<u32>().ok().map(|num| (num, t.to_string()))
                } else {
                    None
                }
            })
            .collect();

        parsed_tags.sort_by_key(|(num, _)| *num);

        if let Some((_, max_tag)) = parsed_tags.last() {
            return Ok(max_tag.clone());
        }
    }

    Ok("b10355".to_string())
}

#[tauri::command]
async fn get_server_version(server_path: String) -> Result<String, String> {
    let raw_path = server_path.trim();
    let exe_path = if raw_path.is_empty() {
        let root_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        if root_dir.join("llama-server.exe").exists() {
            root_dir.join("llama-server.exe")
        } else if std::path::Path::new("llama-server.exe").exists() {
            PathBuf::from("llama-server.exe")
        } else {
            PathBuf::from("llama-server.exe")
        }
    } else {
        let p = PathBuf::from(raw_path);
        if p.is_relative() {
            let root_dir = std::env::current_exe()
                .ok()
                .and_then(|parent| parent.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."));
            if root_dir.join(&p).exists() {
                root_dir.join(&p)
            } else if std::path::Path::new(&p).exists() {
                PathBuf::from(&p)
            } else {
                p
            }
        } else {
            p
        }
    };

    let canonical = exe_path.canonicalize().unwrap_or(exe_path.clone());
    let display_path = canonical.to_string_lossy().replace(r"\\?\", "");

    let output = Command::new(&exe_path)
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("无法调起可执行文件 ({}) : {}", display_path, e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let ver_info = if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else {
        "已调起但未返回版本字符串".to_string()
    };

    Ok(format!("[{}]\n{}", display_path, ver_info))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerPid(Mutex::new(None)))
        .manage(ProxyHandle(Mutex::new(None)))
        .manage(ServerPort(Mutex::new(None)))
        .manage(CompileChild(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            start_server,
            stop_server,
            load_builtins,
            load_profiles,
            save_profiles,
            open_log,
            browse_file,
            get_bundled_server_path,
            inspect_gguf,
            estimate_vram_budget,
            check_build_env,
            start_compile_engine,
            cancel_compile_engine,
            get_latest_llama_tag,
            get_server_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
