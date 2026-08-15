# build_llama-server.ps1 - LLaMA Launcher 本地极速编译脚本
param(
    [string]$SourceDir   = "llama-source",
    [string]$BuildNumber = "",
    [string]$CudaVersion = "12.8",
    [string]$CudaArch    = "120",
    [string]$CpuArch     = "avx2",
    [int]$Threads        = 16
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding          = [System.Text.Encoding]::UTF8

$oldEap = $ErrorActionPreference
$ErrorActionPreference = "SilentlyContinue"

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host " LLaMA Server 推理极速编译引擎" -ForegroundColor Green
Write-Host " CUDA: $CudaVersion | CUDA Arch: sm_$CudaArch | CPU Arch: $CpuArch | 线程数: $Threads" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""

# ============================================================
# 0. 自动加载 Visual Studio 环境 (vcvars64.bat)
# ============================================================
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsPath = ""
if (Test-Path $vswhere) {
    $vsPath = & $vswhere -products * -latest -property installationPath 2>$null
}
if (-not $vsPath) {
    $vsSearch = @(
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools",
        "C:\Program Files\Microsoft Visual Studio\2022\Community",
        "C:\Program Files\Microsoft Visual Studio\2022\Professional",
        "C:\Program Files\Microsoft Visual Studio\2022\Enterprise",
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\Community"
    )
    foreach ($p in $vsSearch) {
        if (Test-Path "$p\VC\Auxiliary\Build\vcvars64.bat") {
            $vsPath = $p
            break
        }
    }
}

if ($vsPath) {
    $vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"
    if (Test-Path $vcvars) {
        cmd.exe /c "`"$vcvars`" && set" | ForEach-Object {
            if ($_ -match '^(.*?)=(.*)$') {
                Set-Item -Path "env:$($matches[1])" -Value $matches[2]
            }
        }
        Write-Host "0/5 已成功加载 MSVC x64 编译环境 ($vsPath)" -ForegroundColor Cyan
    }
}

# 优先加载 ccache
$customCcacheDirs = @(
    "C:\Tools\Ccache\ccache-4.13.6-windows-x86_64",
    "C:\Tools\Ccache"
)
foreach ($dir in $customCcacheDirs) {
    if (Test-Path "$dir\ccache.exe") {
        $env:PATH = "$dir;$env:PATH"
        break
    }
}

# ============================================================
# 1. 动态配置 CUDA Toolkit 版本
# ============================================================
if ($CudaVersion -match '12\.8|13\.2|12|13') {
    $cudaRoot = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CudaVersion"
    if (Test-Path $cudaRoot) {
        $env:CUDA_PATH = $cudaRoot
        $env:PATH = "$cudaRoot\bin;$env:PATH"
        Write-Host "0/5 已切换至 CUDA Toolkit $CudaVersion ($cudaRoot)" -ForegroundColor Cyan
    } else {
        Write-Host "0/5 警告: 未找到 $cudaRoot，将使用默认系统 CUDA_PATH ($env:CUDA_PATH)" -ForegroundColor Yellow
    }
}

$cudaNvcc = "$env:CUDA_PATH\bin\nvcc.exe"
if (Test-Path $cudaNvcc) {
    $nvccVer = & $cudaNvcc --version 2>&1 | Select-String "release"
    Write-Host "        当前 NVCC 编译器: $nvccVer" -ForegroundColor Cyan
}

# ============================================================
# 2. 定位与处理任意名称的源码目录
# ============================================================
$realSourceDir = ""
if (Test-Path $SourceDir) {
    $realSourceDir = (Get-Item $SourceDir).FullName
    Write-Host "1/5 使用自定义本地源码目录: $realSourceDir" -ForegroundColor Cyan
} elseif (Test-Path "llama-source") {
    $realSourceDir = (Get-Item "llama-source").FullName
    Write-Host "1/5 使用现有源码目录: $realSourceDir" -ForegroundColor Cyan
} else {
    Write-Host "1/5 正在克隆 llama.cpp 官方源码到 $SourceDir ..." -ForegroundColor Cyan
    $parentDir = Split-Path $SourceDir -Parent
    if ($parentDir -and (-not (Test-Path $parentDir))) {
        New-Item -ItemType Directory -Path $parentDir -Force | Out-Null
    }
    
    # 解析可用的分支/Tag 标签 (如 b10355)
    $branchOpt = ""
    $m = [regex]::Match($SourceDir, 'b?(\d{4,5})')
    if ($m.Success) {
        $branchOpt = "b$($m.Groups[1].Value)"
    }
    
    if ($branchOpt) {
        Write-Host "        尝试直接抓取 Tag: $branchOpt ..." -ForegroundColor Cyan
        git clone --branch $branchOpt --depth 1 https://github.com/ggml-org/llama.cpp.git $SourceDir 2>$null
    }
    
    if (-not (Test-Path $SourceDir)) {
        git clone --depth 50 https://github.com/ggml-org/llama.cpp.git $SourceDir
    }
    
    if ($LASTEXITCODE -ne 0 -and (-not (Test-Path $SourceDir))) { throw "git clone 失败，请检查网络" }
    $realSourceDir = (Get-Item $SourceDir).FullName
}

# ============================================================
# 3. 解析构建版本号 (Build Number)
# ============================================================
$tagNum = ""
$commitHash = ""

if ($BuildNumber) {
    $m = [regex]::Match($BuildNumber, '\d+')
    if ($m.Success) { $tagNum = $m.Groups[0].Value }
}

if (-not $tagNum) {
    # 尝试从源码目录名正则解析 (如 llama.cpp-b10326 / b9902)
    $dirName = (Get-Item $realSourceDir).Name
    $dirMatch = [regex]::Match($dirName, 'b?(\d{4,5})')
    if ($dirMatch.Success) {
        $tagNum = $dirMatch.Groups[1].Value
    }
}

if (-not $tagNum) {
    # 尝试从 git 提取
    if (Test-Path "$realSourceDir\.git") {
        Push-Location $realSourceDir
        try {
            $describe = (git describe --tags --always 2>$null)
            if ($describe) {
                $tagMatch = [regex]::Match($describe, 'b?(\d+)')
                if ($tagMatch.Success) { $tagNum = $tagMatch.Groups[1].Value }
            }
            $commitHash = (git rev-parse --short HEAD 2>$null)
        } finally {
            Pop-Location
        }
    }
}

if (-not $tagNum) { $tagNum = "9902" }
if (-not $commitHash) { $commitHash = "b$tagNum" }

Write-Host "2/5 成功锁定本地编译版本: Build #$tagNum ($commitHash)" -ForegroundColor Cyan

# ============================================================
# 4. Patch llama-server-impl -> STATIC
# ============================================================
$serverCMake = Join-Path $realSourceDir "tools\server\CMakeLists.txt"
if (Test-Path $serverCMake) {
    $lines = Get-Content $serverCMake
    $newLines = [System.Collections.Generic.List[string]]::new()
    $foundTarget = $false
    $patchDone = $false

    foreach ($line in $lines) {
        if ($line -match 'set\(TARGET llama-server-impl\)') {
            $foundTarget = $true
            $newLines.Add($line)
            continue
        }
        if ($foundTarget -and $line -match '^add_library\(\$\{TARGET\}\s*$') {
            $newLines.Add('add_library(${TARGET} STATIC')
            $foundTarget = $false
            $patchDone = $true
            continue
        }
        if ($line -match 'WINDOWS_EXPORT_ALL_SYMBOLS ON') {
            $newLines.Add('# WINDOWS_EXPORT_ALL_SYMBOLS removed for STATIC build')
            continue
        }
        if ($foundTarget -and $line -match 'add_library') {
            $foundTarget = $false
        }
        $newLines.Add($line)
    }

    if ($patchDone) {
        Set-Content -Path $serverCMake -Value $newLines -Encoding UTF8
        Write-Host "3/5 Patch 成功: llama-server-impl 强制静态链接化" -ForegroundColor Cyan
    }
}

# ============================================================
# 5. CMake 配置
# ============================================================
$buildDir = Join-Path $realSourceDir "build"
if (Test-Path $buildDir) {
    Remove-Item -Path $buildDir -Recurse -Force
}
New-Item -ItemType Directory -Path $buildDir | Out-Null

$cmakeArgs = @(
    "..",
    "-DCMAKE_BUILD_TYPE=Release",
    "-DLLAMA_BUILD_NUMBER=$tagNum",
    "-DLLAMA_BUILD_COMMIT=$commitHash",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_CUDA=ON",
    "-DCMAKE_CUDA_ARCHITECTURES=$CudaArch",
    "-DGGML_CUDA_FORCE_CUBLAS=OFF",
    "-DGGML_CUDA_FA=ON",
    "-DGGML_CUDA_FA_ALL_QUANTS=ON",
    "-DGGML_CUDA_GRAPHS=ON",
    "-DGGML_OPENMP=ON",
    "-DLLAMA_BUILD_EXAMPLES=OFF",
    "-DLLAMA_BUILD_TESTS=OFF",
    "-DLLAMA_BUILD_SERVER=ON"
)

if (Test-Path $cudaNvcc) {
    $cmakeArgs += "-DCMAKE_CUDA_COMPILER=$cudaNvcc"
}

# CPU 架构设置
if ($CpuArch -eq "avx512") {
    $cmakeArgs += "-DGGML_AVX512=ON"
    $cmakeArgs += "-DGGML_AVX2=ON"
    $cmakeArgs += "-DGGML_FMA=ON"
    $cmakeArgs += "-DGGML_F16C=ON"
} elseif ($CpuArch -eq "avx") {
    $cmakeArgs += "-DGGML_AVX=ON"
    $cmakeArgs += "-DGGML_AVX2=OFF"
} elseif ($CpuArch -eq "generic") {
    $cmakeArgs += "-DGGML_NATIVE=OFF"
    $cmakeArgs += "-DGGML_AVX=OFF"
    $cmakeArgs += "-DGGML_AVX2=OFF"
    $cmakeArgs += "-DGGML_FMA=OFF"
    $cmakeArgs += "-DGGML_F16C=OFF"
} else {
    # 默认 AVX2
    $cmakeArgs += "-DGGML_AVX2=ON"
    $cmakeArgs += "-DGGML_FMA=ON"
    $cmakeArgs += "-DGGML_F16C=ON"
}

Write-Host "3/5 正在配置 CMake 参数 (CUDA sm_$CudaArch / CPU $CpuArch)..." -ForegroundColor Cyan

Push-Location $buildDir
try {
    $oldEap = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    $cmakeOut = & cmake.exe @cmakeArgs 2>&1
    $ErrorActionPreference = $oldEap
    if ($LASTEXITCODE -ne 0) {
        Write-Host "==========================================" -ForegroundColor Red
        Write-Host "CMake 配置失败！详细日志如下：" -ForegroundColor Red
        Write-Host $cmakeOut
        Write-Host "==========================================" -ForegroundColor Red
        throw "CMake 配置失败，退出码 $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

# ============================================================
# 6. 多核极速编译
# ============================================================
Write-Host "4/5 正在进行多核极速编译 ($Threads 线程)..." -ForegroundColor Cyan

Push-Location $buildDir
try {
    $oldEap = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    $buildOut = & cmake.exe --build . --config Release --target llama-server --parallel $Threads -- /v:q /nologo 2>&1
    $ErrorActionPreference = $oldEap
    if ($LASTEXITCODE -ne 0) {
        Write-Host "==========================================" -ForegroundColor Red
        Write-Host "编译失败！详细日志如下：" -ForegroundColor Red
        Write-Host $buildOut
        Write-Host "==========================================" -ForegroundColor Red
        throw "编译失败，退出码 $LASTEXITCODE"
    }
    Write-Host "        编译 100% 完成！" -ForegroundColor Cyan
} finally {
    Pop-Location
}

# ============================================================
# 7. 提取产物并拷贝到启动器目录
# ============================================================
$exe = Get-ChildItem -Path $buildDir -Recurse -Filter "llama-server.exe" | Where-Object { $_.Length -gt 1MB } | Select-Object -First 1
if (-not $exe) {
    throw "未能在 $buildDir 中找到有效编译产物 llama-server.exe"
}

$appRoot = (Get-Item $PSScriptRoot).FullName
if ((Get-Item $appRoot).Name -eq "logs") {
    $parentDir = (Get-Item $appRoot).Parent.FullName
    $appRoot = $parentDir
}

$destExe = Join-Path $appRoot "llama-server.exe"

# 1. 如果存在旧的 llama-server.exe，通过 --version 获取其版本并重命名备份为 llama-server_bxxxxx.exe
if (Test-Path $destExe) {
    $oldTag = ""
    try {
        $oldVerOut = & $destExe --version 2>&1
        $oldVerStr = ($oldVerOut | Out-String)
        $m1 = [regex]::Match($oldVerStr, 'version:\s*(\d+)')
        if ($m1.Success) {
            $oldTag = "b$($m1.Groups[1].Value)"
        } else {
            $m2 = [regex]::Match($oldVerStr, 'b?(\d{4,5})')
            if ($m2.Success) {
                $oldTag = "b$($m2.Groups[1].Value)"
            }
        }
    } catch { }

    if (-not $oldTag) { $oldTag = "b_old" }

    $backupName = "llama-server_${oldTag}.exe"
    $backupPath = Join-Path $appRoot $backupName
    Write-Host "5/5 检测到已有内核，正在将其备份归档为: $backupName ..." -ForegroundColor Cyan
    Copy-Item $destExe -Destination $backupPath -Force
}

# 2. 将新编译好的 llama-server.exe 拷贝到目标路径
Copy-Item $exe.FullName -Destination $destExe -Force

# 3. 另外存一份带新版本号的命名文件 (如 llama-server_b10423.exe)
$newVersionedExe = Join-Path $appRoot "llama-server_b${tagNum}.exe"
Copy-Item $exe.FullName -Destination $newVersionedExe -Force

$resDir = Join-Path $appRoot "src-tauri\resources"
if (Test-Path $resDir) {
    Copy-Item $exe.FullName -Destination (Join-Path $resDir "llama-server.exe") -Force
}

$cublasDll = Get-ChildItem -Path "$env:CUDA_PATH\bin" -Filter "cublas64_*.dll" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($cublasDll) {
    Copy-Item $cublasDll.FullName -Destination (Join-Path $appRoot $cublasDll.Name) -Force
    if (Test-Path $resDir) {
        Copy-Item $cublasDll.FullName -Destination (Join-Path $resDir $cublasDll.Name) -Force
    }
}

Write-Host "5/5 编译成功！新内核已自动替换到应用执行路径: $destExe" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Green
Write-Host "恭喜！Build #$tagNum 编译成功！" -ForegroundColor Green
Write-Host "==========================================" -ForegroundColor Green

& $destExe --version
