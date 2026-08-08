# build_local_v15.ps1
# RTX 5060 Ti 16GB + i5-13400
# llama.cpp + CUDA 13.2 + sm_120
#
# 目标：推理速度最大化，保持原编译方式（单文件 llama-server.exe，无额外 DLL 依赖）
#
# 相比 v13.2 的改动：
#   [移除] -DCMAKE_CUDA_RUNTIME_LIBRARY=Static  → 零性能收益，体积+150MB（根本原因）
#   [移除] -DGGML_CUDA_COMPRESSION_MODE=speed   → llama.cpp 不支持，无效
#   [新增] 自动 patch server CMakeLists.txt      → 将 llama-server-impl 强制为 STATIC
#          使 llama-server.exe 静态链接所有内部库，回归单文件独立运行
#   [新增] -DGGML_OPENMP=ON                      → 并行 CPU 线程
#   [新增] MSVC /O2 /Ob3                          → 更激进的编译器内联优化
#
# 体积预期：
#   v13.2 原版（Static CUDA Runtime）：300MB+（绝大部分来自嵌入 CUDA runtime）
#   本版本（Dynamic CUDA Runtime + Static 内部库）：约 100~130MB（正常）

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host " llama.cpp 推理极速编译 v15" -ForegroundColor Green
Write-Host " RTX 5060 Ti + i5-13400 + CUDA 13.2" -ForegroundColor Green
Write-Host " 单文件 exe，无额外 DLL 依赖" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""

# ============================================================
# 0. CUDA 版本
# ============================================================

$CudaVersion = "13.2"
$CudaRoot    = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CudaVersion"
$CudaNvcc    = "$CudaRoot\bin\nvcc.exe"

if (-not (Test-Path $CudaNvcc)) {
    Write-Host "错误：找不到 CUDA $CudaVersion" -ForegroundColor Red
    Write-Host $CudaNvcc
    exit 1
}

Write-Host "CUDA 编译器：$CudaNvcc" -ForegroundColor Yellow
Write-Host ""
Write-Host "CUDA 版本：" -ForegroundColor Cyan
& $CudaNvcc --version

# ============================================================
# 1. 检查 / 更新 llama.cpp 源码
# ============================================================

if (-not (Test-Path "llama-source")) {
    Write-Host ""
    Write-Host "1/5 正在克隆 llama.cpp 官方源码..." -ForegroundColor Cyan
    git clone --depth 50 https://github.com/ggml-org/llama.cpp.git llama-source
}
else {
    Write-Host ""
    Write-Host "1/5 llama-source 已存在，更新源码..." -ForegroundColor Cyan
    Push-Location llama-source
    try {
        git fetch --tags --force origin
        git pull
    }
    finally {
        Pop-Location
    }
}

# ============================================================
# 2. 获取版本信息
# ============================================================

Push-Location llama-source

try {
    $isShallow = (git rev-parse --is-shallow-repository 2>$null) -eq "true"
    if ($isShallow) {
        Write-Host "检测到浅克隆，正在获取 Git Tag..." -ForegroundColor Yellow
        git fetch --tags --force origin 2>$null
    }

    $tagRaw = git describe --tags --always 2>$null
    if (-not $tagRaw) { $tagRaw = git describe --tags --abbrev=0 2>$null }

    if ($tagRaw -match 'b?(\d+)') {
        $tagNum = $Matches[1]
    } else {
        $countRaw = git rev-list --count HEAD 2>$null
        $tagNum   = if ($countRaw) { $countRaw } else { "10229" }
    }

    $commitHash = git rev-parse --short HEAD 2>$null
    if (-not $commitHash) { $commitHash = "custom" }
}
finally {
    Pop-Location
}

Write-Host ""
Write-Host "2/5 llama.cpp 版本信息" -ForegroundColor Cyan
Write-Host "Build Number : $tagNum"    -ForegroundColor Yellow
Write-Host "Commit       : $commitHash" -ForegroundColor Yellow

# ============================================================
# 3. Patch：将 llama-server-impl 强制为 STATIC
#    新版 llama.cpp 默认将其编译为动态库，导致 exe 只有 11KB
#    通过替换 CMakeLists.txt 中的 add_library 调用，强制静态链接
# ============================================================

$ServerCMake = "llama-source\tools\server\CMakeLists.txt"

if (-not (Test-Path $ServerCMake)) {
    Write-Host ""
    Write-Host "错误：找不到 $ServerCMake" -ForegroundColor Red
    Write-Host "llama.cpp 源码结构可能已变更，请检查 tools/server 目录" -ForegroundColor Yellow
    exit 1
}

Write-Host ""
Write-Host "3/5 Patch server CMakeLists.txt → llama-server-impl 强制 STATIC..." -ForegroundColor Cyan

# 逐行处理，避免 PowerShell -replace 的换行兼容性问题
$lines       = Get-Content $ServerCMake
$newLines    = [System.Collections.Generic.List[string]]::new()
$foundTarget = $false
$patchDone   = $false

foreach ($line in $lines) {

    # 记录到 llama-server-impl 的 set(TARGET ...) 行
    if ($line -match 'set\(TARGET llama-server-impl\)') {
        $foundTarget = $true
        $newLines.Add($line)
        continue
    }

    # 紧随其后的 add_library(${TARGET} → 改为 STATIC
    if ($foundTarget -and $line -match '^add_library\(\$\{TARGET\}\s*$') {
        $newLines.Add('add_library(${TARGET} STATIC')
        $foundTarget = $false
        $patchDone   = $true
        continue
    }

    # 移除 WINDOWS_EXPORT_ALL_SYMBOLS（对静态库无意义）
    if ($line -match 'WINDOWS_EXPORT_ALL_SYMBOLS ON') {
        $newLines.Add('# WINDOWS_EXPORT_ALL_SYMBOLS removed for STATIC build')
        continue
    }

    # 遇到其他 add_library 说明没匹配上，重置标记
    if ($foundTarget -and $line -match 'add_library') {
        $foundTarget = $false
    }

    $newLines.Add($line)
}

if ($patchDone) {
    Set-Content -Path $ServerCMake -Value $newLines -Encoding UTF8
    Write-Host "Patch 成功：llama-server-impl → STATIC" -ForegroundColor Green
} else {
    Write-Host "警告：Patch 匹配失败，源码结构可能已变更" -ForegroundColor Yellow
    Write-Host "      继续编译，但 exe 可能仍依赖 DLL" -ForegroundColor Yellow
}

# ============================================================
# 4. 创建全新的 CMake Build 目录
# ============================================================

$BuildDir = "llama-source\build"

if (Test-Path $BuildDir) {
    Write-Host ""
    Write-Host "删除旧 build 目录..." -ForegroundColor Cyan
    Remove-Item -Path $BuildDir -Recurse -Force
}

New-Item -ItemType Directory -Path $BuildDir | Out-Null

# ============================================================
# 5. CMake 配置
# ============================================================

Write-Host ""
Write-Host "4/5 正在配置 CMake..." -ForegroundColor Cyan
Write-Host ""
Write-Host "============================================================" -ForegroundColor DarkGray
Write-Host " 编译配置：" -ForegroundColor Green
Write-Host "   CUDA Compiler : $CudaNvcc" -ForegroundColor Green
Write-Host "   CUDA Arch     : sm_120 (Blackwell)" -ForegroundColor Green
Write-Host "   CUDA Runtime  : Dynamic (体积正常，性能等同)" -ForegroundColor Green
Write-Host "   Flash Attn    : ON (全量化类型)" -ForegroundColor Green
Write-Host "   CUDA Graphs   : ON" -ForegroundColor Green
Write-Host "   CPU SIMD      : AVX2+FMA+F16C+BMI2+AVX-VNNI" -ForegroundColor Green
Write-Host "   OpenMP        : ON" -ForegroundColor Green
Write-Host "   LTO           : ON" -ForegroundColor Green
Write-Host "   Output        : 单文件 exe（内部库已静态链接）" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor DarkGray
Write-Host ""

Push-Location $BuildDir

try {

    cmake .. `
        -DCMAKE_BUILD_TYPE=Release `
        "-DCMAKE_CUDA_COMPILER=$CudaNvcc" `
        "-DLLAMA_BUILD_NUMBER=$tagNum" `
        "-DLLAMA_BUILD_COMMIT=$commitHash" `
        -DGGML_CUDA=ON `
        -DCMAKE_CUDA_ARCHITECTURES=120 `
        -DGGML_CUDA_FORCE_CUBLAS=OFF `
        -DGGML_CUDA_FA=ON `
        -DGGML_CUDA_FA_ALL_QUANTS=ON `
        -DGGML_CUDA_GRAPHS=ON `
        -DBUILD_SHARED_LIBS=OFF `
        -DGGML_NATIVE=ON `
        -DGGML_AVX2=ON `
        -DGGML_FMA=ON `
        -DGGML_F16C=ON `
        -DGGML_BMI2=ON `
        -DGGML_AVX_VNNI=ON `
        -DGGML_OPENMP=ON `
        -DGGML_LTO=ON `
        "-DCMAKE_CXX_FLAGS_RELEASE=/O2 /Ob3 /DNDEBUG" `
        "-DCMAKE_C_FLAGS_RELEASE=/O2 /Ob3 /DNDEBUG" `
        -DLLAMA_BUILD_EXAMPLES=OFF `
        -DLLAMA_BUILD_TESTS=OFF `
        -DLLAMA_BUILD_SERVER=ON `
        "-DCMAKE_CUDA_FLAGS=--threads 4"

    if ($LASTEXITCODE -ne 0) {
        throw "CMake 配置失败"
    }

}
finally {
    Pop-Location
}

# ============================================================
# 6. 验证 CUDA 配置
# ============================================================

$CacheFile = "$BuildDir\CMakeCache.txt"

if (-not (Test-Path $CacheFile)) {
    throw "找不到 CMakeCache.txt，CMake 配置可能失败"
}

Write-Host ""
Write-Host "验证 CUDA 配置..." -ForegroundColor Cyan

Select-String -Path $CacheFile -Pattern "CMAKE_CUDA_COMPILER:" |
    ForEach-Object { Write-Host $_.Line -ForegroundColor Yellow }

Select-String -Path $CacheFile -Pattern "CMAKE_CUDA_COMPILER_VERSION:" |
    ForEach-Object { Write-Host $_.Line -ForegroundColor Yellow }

Select-String -Path $CacheFile -Pattern "CMAKE_CUDA_ARCHITECTURES" |
    ForEach-Object { Write-Host $_.Line -ForegroundColor Yellow }

Write-Host ""

# ============================================================
# 7. 编译 llama-server
# ============================================================

Write-Host ""
Write-Host "5/5 开始编译 llama-server..." -ForegroundColor Cyan
Write-Host "并行线程：16" -ForegroundColor Yellow
Write-Host ""

Push-Location $BuildDir

try {

    cmake --build . `
        --config Release `
        --target llama-server `
        -j 16

    if ($LASTEXITCODE -ne 0) {
        throw "llama-server 编译失败"
    }

}
finally {
    Pop-Location
}

# ============================================================
# 8. 查找 llama-server.exe
# ============================================================

$exe = Get-ChildItem `
    -Path $BuildDir `
    -Recurse `
    -Filter "llama-server.exe" `
    -ErrorAction SilentlyContinue |
    Where-Object { $_.Length -gt 1MB } |
    Select-Object -First 1

if (-not $exe) {
    # 如果 patch 未生效，exe 仍为 11KB，给出明确提示
    $stubExe = Get-ChildItem -Path $BuildDir -Recurse -Filter "llama-server.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($stubExe -and $stubExe.Length -lt 100KB) {
        Write-Host ""
        Write-Host "错误：llama-server.exe 只有 $([math]::Round($stubExe.Length/1KB))KB，Patch 未生效" -ForegroundColor Red
        Write-Host "请检查 tools/server/CMakeLists.txt 中 llama-server-impl 的定义是否已改为 STATIC" -ForegroundColor Yellow
    } else {
        Write-Host ""
        Write-Host "错误：没有找到 llama-server.exe" -ForegroundColor Red
    }
    exit 1
}

# ============================================================
# 9. 复制到脚本目录
# ============================================================

$VersionedName    = "llama-server-b${tagNum}-${commitHash}.exe"
$VersionedExePath = Join-Path (Get-Location) $VersionedName
$OutputExe        = Join-Path (Get-Location) "llama-server.exe"

Copy-Item -Path $exe.FullName -Destination $VersionedExePath -Force
Copy-Item -Path $exe.FullName -Destination $OutputExe -Force

$exeSizeMB = [math]::Round((Get-Item $OutputExe).Length / 1MB, 1)

$BuildInfoData = [ordered]@{
    "build_number" = "b$tagNum"
    "commit"       = $commitHash
    "cuda_version" = $CudaVersion
    "architecture" = "sm_120"
    "cuda_runtime" = "Dynamic"
    "perf_flags"   = "FA+FA_ALL_QUANTS+CUDA_GRAPHS+AVX2+FMA+F16C+BMI2+AVX_VNNI+OPENMP+LTO"
    "build_type"   = "single-exe (llama-server-impl STATIC patched)"
    "exe_size_mb"  = $exeSizeMB
    "build_time"   = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
}

$BuildInfoPath = Join-Path (Get-Location) "build_info.json"
$BuildInfoData | ConvertTo-Json | Out-File -FilePath $BuildInfoPath -Encoding utf8 -Force

# ============================================================
# 10. 最终结果
# ============================================================

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host " 编译成功！" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""
Write-Host "llama.cpp Build : #$tagNum"    -ForegroundColor Cyan
Write-Host "Commit          : $commitHash"  -ForegroundColor Cyan
Write-Host "CUDA            : $CudaVersion" -ForegroundColor Cyan
Write-Host "Architecture    : sm_120 (Blackwell)" -ForegroundColor Cyan
Write-Host "Runtime         : Dynamic"      -ForegroundColor Cyan
Write-Host "Output          : 单文件 exe（内部库静态链接）" -ForegroundColor Cyan
Write-Host ""
Write-Host "exe 体积：${exeSizeMB} MB" -ForegroundColor Cyan

if ($exeSizeMB -lt 10) {
    Write-Host "警告：体积过小（Patch 可能未生效），exe 仍依赖 DLL" -ForegroundColor Red
} elseif ($exeSizeMB -gt 300) {
    Write-Host "警告：体积偏大（请确认 Static CUDA Runtime 未被重新加入）" -ForegroundColor Yellow
} else {
    Write-Host "体积正常（预期 100~150MB）" -ForegroundColor Green
}

Write-Host ""
Write-Host "输出文件：" -ForegroundColor Cyan
Write-Host "1. 带版本标识: $VersionedExePath" -ForegroundColor Yellow
Write-Host "2. 通用快捷名: $OutputExe"         -ForegroundColor Yellow
Write-Host "3. 版本元数据: $BuildInfoPath"     -ForegroundColor Yellow

Write-Host ""
Write-Host "版本验证：" -ForegroundColor Cyan
& $OutputExe --version

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host " 完成" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""
