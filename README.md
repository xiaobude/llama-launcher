# llama-launcher
Windows 本地 `llama-server` 图形化启动器,内置 Anthropic Messages API 兼容代理,可让只支持 Anthropic API 的客户端（如 Claude Code）直接接入本地 llama-server。

# LLaMA Launcher Pro

**下载地址**
- Llama-server 运行文件：[ggml-org/llama.cpp](https://github.com/ggml-org/llama.cpp)
- 开源本地模型：[Hugging Face Models](https://huggingface.co/models)

---

Windows 本地 `llama-server` 图形化启动器。已从 HTA (HTML Application) 迁移至 **Tauri 2.0** 桌面应用，无需 Node.js、Python 或 .NET 等额外运行时，双击 `.exe` 即可运行。新增内置 **Anthropic Messages API 兼容代理**，可让只支持 Anthropic API 的客户端（如 Claude Code）直接接入本地 llama-server。

## 版本历史

| 版本 | 技术栈 | 主文件 |
|:---|:---|:---|
| v1 (HTA) | HTA + WScript + PowerShell | `llama-launcher.hta` |
| v2 (Tauri 1) | Rust + Tauri v1 + 纯静态 HTML/CSS/JS | `LLaMA 启动器 Pro.exe` |
| v3 (Tauri 2 + Anthropic 代理) | Rust + Tauri 2.0 + axum 内嵌代理 | `LLaMA 启动器 Pro.exe` |

---

## 📁 项目文件结构

### 运行目录（exe 所在目录）

```text
<安装目录>/
├── LLaMA 启动器 Pro.exe   # 主程序（双击运行）
├── config.json            # 内置配置预设（只读，随安装包分发）
└── profiles.json          # 用户自定义配置（自动生成，支持保存与删除）
```

> **日志目录**：运行时日志写入 `%APPDATA%\com.getai.llama-launcher\logs\llama-server.log`，不在 exe 旁边。

### 源码目录

```text
llama-launcher-tauri/
├── dist/                  # 前端（纯静态，无构建步骤）
│   ├── index.html
│   ├── style.css
│   └── main.js
├── src-tauri/
│   ├── src/main.rs              # Rust 后端（Tauri 命令）
│   ├── src/anthropic_proxy.rs   # Anthropic Messages API 兼容代理（axum）
│   ├── tauri.conf.json          # 应用配置（窗口、bundle、capabilities）
│   └── Cargo.toml
└── Cargo.toml             # workspace 根
```

---

## 🚀 快速开始

1. **启动应用**：双击 `LLaMA 启动器 Pro.exe`（安装版）或直接运行 debug 构建 `target\debug\LLaMA 启动器 Pro.exe`。
2. **选择/配置预设**：
   - 从右上角下拉菜单选择内置配置（如 `VL-通用`、`c64k` 等）或自定义配置。
   - 可手动调整各项参数，或点击右侧 **📁** 按钮通过系统原生文件选择器浏览 `llama-server.exe` 路径、`.gguf` 模型文件和视觉模型。
3. **管理配置**：
   - 点击 **💾 保存** 可将当前参数持久化为自定义预设到 `profiles.json`。若选中的是内置预设，会提示重命名另存，防止覆盖只读内容。
   - 点击 **🗑 删除** 可清除当前选中的自定义预设（不可删除内置预设）。
4. **启动服务**：
   - ⚠️ **若需要使用 Anthropic API（`/v1/messages`），必须在点击启动服务之前先勾选 启用 Anthropic API 兼容**，否则代理不会随服务一起拉起，且启动后中途勾选不会生效，必须停止服务后重新启动。
   - 点击 **▶ 启动服务**，启动器执行环境检测、清理残留并在后台拉起 `llama-server` 进程。
   - 状态栏显示 `⏳ 模型加载中 (xx.x%)...`，模型载入完成后变为绿色 `✅ 服务就绪 · 点击查看日志`。
5. **实时日志流**：
   - 点击绿色状态栏，会弹出 PowerShell 窗口实时滚动监视服务器日志（`Get-Content -Wait`）。
6. **停止服务**：
   - 点击 **■ 停止** 按钮终止当前 llama-server 进程。

---

## ⚙️ config.json & profiles.json 配置规范

### 配置存储逻辑

| 文件 | 用途 | 位置 | 可编辑 |
|:---|:---|:---|:---:|
| `config.json` | 内置只读预设（随安装包分发） | exe 同目录 | ❌ |
| `profiles.json` | 用户自定义配置 | exe 同目录（首次保存时自动创建） | ✅ |

- 启动时同时加载两个文件，下拉菜单先显示内置预设，再显示自定义配置。
- 内置预设不可覆盖、不可删除；若用户以内置预设名称保存，会提示重命名。
- 文件编码必须为 **UTF-8 无 BOM**；带 BOM 的文件会导致 JSON 解析静默失败。
- 启动器在 exe 目录及向上最多 5 层父目录中自动搜索配置文件（支持将 exe 拷贝至新目录后仍能找到配置）。

### 📋 完整配置字段与命令行映射表

| UI标签 / 字段名 | 命令行参数 / 环境变量映射 | 类型 | 说明与示例 |
|:---|:---|:---:|:---|
| **服务器路径**<br>`serverPath` | *（可执行程序路径）* | `string` | 本地 `llama-server.exe` 的绝对路径。 |
| **主模型路径**<br>`modelPath` | `-m <path>` | `string` | 主模型 GGUF 文件路径。支持按文件名自动联动投机解码类型。 |
| **外部草稿模型**<br>`draftModelPath` | `--model-draft <path>` | `string` | 外部小草稿模型路径，用于普通 speculative decoding 加速。 |
| **启用视觉**<br>`mmprojEnabled` | *（控制视觉输入与置灰）* | `boolean`| 是否启用多模态 Vision 支持。若勾选，则可选择 `mmproj` 文件。 |
| **mmproj (Vision)**<br>`mmprojPath` | `--mmproj <path>` | `string` | 多模态投影层文件路径。 |
| **别名**<br>`alias` | `--alias <name>` | `string` | 模型在 API 中的名称标识（对应 API 的 `model` 字段），默认为 `local-model` |
| **端口**<br>`port` | `--port <port>` | `number` | 服务监听端口，默认 `8080`。 |
| **允许局域网访问**<br>`host` | `--host 0.0.0.0` | `string` | 若开启，则绑定 `0.0.0.0` 允许外网访问；若关闭则仅绑定本地。 |
| **CUDA**<br>`cudaDevice` | `set CUDA_VISIBLE_DEVICES=<val>`| `string` | 指定使用的 GPU 设备编号（例如 `"0"`）。非空时，在命令行前设置该环境变量。 |
| **GPU层**<br>`gpuLayers` | `--n-gpu-layers <num>` | `number` | 卸载到 GPU 的模型层数。若显存充足，通常可设置为全部层数或 `-1`。 |
| **上下文**<br>`ctxSize` | `-c <num>` | `number` | 上下文 Token 长度限制（例如 `4096` 或 `24480`）。 |
| **Batch**<br>`batchSize` | `-b <num>` | `number` | 物理批处理大小（Physical Batch Size）。 |
| **uBatch**<br>`ubatchSize` | `-ub <num>` | `number` | 逻辑微批处理大小（Logical Micro-batch Size）。 |
| **-np**<br>`numPhysGpu` | `-np <num>` | `number` | 物理 GPU 数量。 |
| **cacheK** / **cacheV**<br>`cacheK` / `cacheV` | `--cache-type-k`<br>`--cache-type-v` | `string` | 主模型 KV 缓存的精度量化类型（可选 `f16`、`q8_0`、`q4_0`）。 |
| **cacheRAM**<br>`cacheRam` | `--cache-ram <MiB>` | `number` | 分配给 CPU 端 KV 缓存的最大内存大小 (以 MiB 为单位)。 |
| **线程** / **批线程**<br>`threads` / `threadsBatch` | `-t <num>`<br>`-tb <num>` | `number` | 正常推理时使用的 CPU 线程数，以及进行 Batch 计算时使用的线程数。 |
| **jinja**<br>`jinja` | `--jinja` | `boolean`| 启用内置 Jinja2 模板支持（强烈建议开启以获得最佳格式渲染）。 |
| **flashAttn**<br>`flashAttn` | `--flash-attn on` | `boolean`| 启用 Flash Attention 加速，显著降低大上下文下的显存消耗。 |
| **noMmap**<br>`noMmap` | `--no-mmap` | `boolean`| 禁用内存映射（显存充裕或需要防止内存颠簸时开启）。 |
| **kvUnified**<br>`kvUnified` | `--kv-unified` / `--no-kv-unified` | `boolean`| 是否启用统一 KV cache 内存池。 |
| **contBatching**<br>`contBatching` | `--cont-batching` | `boolean`| 启用连续批处理（Continuous Batching，多并发及打字机流畅输出关键）。 |
| **metrics**<br>`metrics` | `--metrics` | `boolean`| 暴露 `/metrics` 监控端点（符合 Prometheus 规范）。 |
| **reasoning**<br>`reasoning` | `--reasoning on` / `--reasoning off` | `boolean`| 显式开启或关闭推理链（Chain of Thought）渲染开关。 |
| **推理预算**<br>`reasoningBudget` | `--reasoning-budget <num>`| `number` | 推理模式下的最大 Token 预算限制。 |
| **投机解码 (内置MTP)**<br>`specType` | `--spec-type <type>` | `string` | 投机解码类型。可选 `none`、`draft-mtp` (限 MTP 权重)、`nfnet`、`ptn`。 |
| **草稿N**<br>`draftN` | `--spec-draft-n-max <num>`| `number` | 投机解码时单次预测的最大草稿 Token 数量（如 `2`）。 |
| **NGL**<br>`draftNgl` | `--spec-draft-ngl <num>` | `number` | 草稿模型中卸载到 GPU 的层数。 |
| **草稿cacheK** / **V**<br>`draftTypeK` / `draftTypeV` | `--spec-draft-type-k`<br>`--spec-draft-type-v`| `string` | 草稿模型 KV 缓存量化精度（可选 `f16`、`q8_0`、`q4_0`）。 |
| **日志级别**<br>`logVerbosity` | `--log-verbosity <num>` | `number` | 日志输出详细程度（可选 `0` 到 `3`）。 |
| **日志格式**<br>`logFormat` | `--log-format <format>` | `string` | 控制台日志格式（可选 `text` 或 `json`）。 |
| **图片minTok**<br>`imageMinTokens` | `--image-min-tokens <num>`| `number` | 多模态模式下图像解析的最小 Token 数量限制。 |
| **额外参数**<br>`extraParams` | *(直接拼接命令行参数)* | `string` | 支持输入多行自定义参数，**每行必须以 `--` 开头**（如 `--temp 0.8`）。 |
| **启用 Anthropic API 兼容**<br>`enableAnthropicProxy` | *（不下发给 llama-server，控制内嵌代理是否启动）* | `boolean` | 启用后，启动服务时会额外起一个内嵌 HTTP 代理，把 Anthropic `/v1/messages` 请求转译为 llama-server 的 OpenAI 接口调用。 |
| **Anthropic 端口**<br>`anthropicProxyPort` | *（代理监听端口，与 `port` 独立）* | `number` | 代理服务监听端口，默认 `8081`。需与主服务 `port` 不同。 |

---

## 🛠 启动器底层运行机制（Tauri 2 版）

### 1. 进程管理（Rust 后端）

启动器通过 Tauri IPC (`window.__TAURI__.core.invoke`) 调用 Rust 后端命令。点击 **▶ 启动服务** 时：
- `start_server` 命令接收参数列表和 CUDA 设备号，通过 `std::process::Command` 无窗口启动 (`CREATE_NO_WINDOW`) 后台进程。
- 进程 PID 存入 Rust 全局状态 (`Mutex<Option<u32>>`)，同时写入 `%APPDATA%\...\logs\server.pid`。
- 日志重定向至 `%APPDATA%\com.getai.llama-launcher\logs\llama-server.log`（`stdout + stderr`）。

### 2. 端口检查与安全检测

服务启动前，前端 JS 通过 `fetch` 向 `http://localhost:<port>/health` 发起探测：
- 若端口已被占用且健康接口返回异常，提示用户并拦截启动。
- 若端口空闲，正常拉起新进程。

### 3. 文件选择器

使用 `tauri-plugin-dialog` 插件 (`app.dialog().file().add_filter(...).blocking_pick_file()`) 弹出系统原生文件选择窗口，支持按扩展名过滤（`.exe`、`.gguf` 等），无需 PowerShell 中转。

### 4. 状态轮询 (Health-check)

服务启动后，前端每 `2000ms` 轮询 `/health` 接口：
- **加载中**：返回 `{"status": "loading", "progress": 0.85}`，状态栏显示 `⏳ 模型加载中 (85.0%)...`。
- **就绪**：返回 `{"status": "ok"}`，状态栏变绿显示 `✅ 服务就绪 · 点击查看日志`，停止轮询。

### 5. 实时日志流

点击绿色状态栏时，`open_log` 命令启动一个独立 PowerShell 窗口：
```powershell
Get-Content '<AppData>\logs\llama-server.log' -Encoding UTF8 -Wait -Tail 200
```
实时追踪服务器输出，便于模型调试和显存排错。

---

## 💬 内置聊天测试面板

启动器底部嵌入即时对话测试区，服务进入 `✅ 就绪` 后自动激活。

### ⚙️ 对话控制参数
- **系统提示词 (System Prompt)**：设定 AI 角色（可选）。
- **温度 (Temperature)**：采样温度，范围 `0.0` - `2.0`。
- **MaxTok**：单次回复最大 Token 数。

### 💡 核心特性

1. **实时速率计算 (Token/s)**：
   流式接收 SSE (`data: {...}`) 并从首个 Token 开始计时，公式：
   $$\text{速度 (t/s)} = \frac{\text{已接收 Token 数量}}{\text{当前时间} - T_0}$$

2. **轻量 Markdown 渲染器**：
   内置 `mdToHtml` 极简解析器，无第三方依赖，支持标题、列表、粗体、斜体、行内代码。

3. **键盘交互**：`Enter` 发送，`Shift + Enter` 换行。

---

## 🔌 API 兼容性说明

`llama-server` 默认兼容 **OpenAI 兼容接口规范**，可直接接入 Chatbox、Cherry Studio、Lobe Chat 等客户端：

- **基础端点**：`http://localhost:<端口>/v1`
- **对话补全**：`POST /v1/chat/completions`（支持 `stream: true`）
- **健康检查**：`GET /health`（含状态与模型加载进度）
- **监控端点**：`GET /metrics`（需开启 `metrics` 参数）

---

## 🔗 Anthropic Messages API 兼容代理

llama-server 原生只提供 OpenAI 兼容接口，部分客户端（如 Claude Code）只支持 Anthropic Messages API。为此启动器内嵌了一个独立的 HTTP 代理：

> ⚠️ **必须先勾选再启动**：代理是否启动由 `start_server` 在拉起 llama-server 时一次性决定，**必须在点击 ▶ 启动服务 之前勾选 启用 Anthropic API 兼容**。服务运行期间中途勾选不会生效，需先 ■ 停止再重新启动才能让代理生效。开启后默认提供 `http://localhost:8081/v1/messages`。

- **开启方式**：界面上勾选 **启用 Anthropic API 兼容**，并设置端口（默认 `8081`），随服务一起启动/停止。
- **基础端点**：`http://localhost:<代理端口>/v1`
- **支持接口**：
  - `POST /v1/messages`（支持 `stream: true` 流式 SSE）
  - `POST /v1/messages/count_tokens`
- **转译逻辑**：代理收到 Anthropic 格式请求后转译为 llama-server 的 `POST /v1/chat/completions`（OpenAI 格式）转发，响应再转译回 Anthropic 格式；非流式与流式均支持。
- **接入示例**（以 Claude Code 为例）：
  ```powershell
  $env:ANTHROPIC_BASE_URL = "http://localhost:8081"
  $env:ANTHROPIC_API_KEY = "dummy"   # 本地代理不校验 key，填任意非空值即可
  claude
  ```
- **已知限制**（设计取舍，非缺陷）：
  - 不支持图片/多模态内容块、`/v1/models` 等枚举接口、`tool_choice` 强制策略细节。
  - `count_tokens` 借用 llama-server 的 `/tokenize` 接口统计 token 数组长度，是近似值，并非精确的 Anthropic tokenizer 结果。
  - 工具调用（`tool_use`）流式输出是等模型生成完整 JSON 后一次性发出，不是逐字符增量推送。

---

## 🏗 开发与构建（Tauri 版）

### 环境依赖

- Rust（通过 [rustup](https://rustup.rs/) 安装）
- Tauri CLI：`cargo install tauri-cli --version "^2.0.0"`（本项目纯 Rust/Cargo 工作区，不依赖 Node.js）
- Windows SDK / MSVC 工具链

### 常用命令

```powershell
# 开发模式（热重载前端，实时 Rust 重编译）
cargo tauri dev

# 生产构建（生成 NSIS 安装包）
cargo tauri build
# 输出：src-tauri\target\release\bundle\nsis\LLaMA 启动器 Pro_0.1.0_x64-setup.exe
```

### 分发

NSIS 安装包支持用户自选安装目录。安装后将 `config.json` 放在与 exe 相同目录下即可使用内置预设；`profiles.json` 由应用在首次保存配置时自动创建。
<<<<<<< HEAD

=======
>>>>>>> 2f05d32 (first commit)
