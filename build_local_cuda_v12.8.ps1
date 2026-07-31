# build_local.ps1 - RTX 5060 Ti + i5-13400 本地极速编译脚本
# 注：cudart / cublasLt 可以做到零 DLL，cublas64_*.dll 在 Windows 上没有静态版可选，
#     会被自动拷贝到 exe 同目录，这是 NVIDIA 的平台限制，不是配置错误。
$ErrorActionPreference = "Stop"
Write-Host "=== 正在启动本地极速编译 (RTX 5060 Ti + i5-13400) ===" -ForegroundColor Green

# 0. 强制本次会话使用 CUDA 12.8 编译（不改动系统级 CUDA_PATH，只影响本脚本进程）
#    原因：CUDA 13.x 在 Blackwell(sm_120) 上会让 MMQ 量化矩阵乘 kernel 崩溃/回退到更慢的 cuBLAS 路径，
#    NVIDIA 官方也建议用 12.8 编译 sm_120。你系统变量 CUDA_PATH 指向的是 v13.2，这里临时覆盖。
$cuda128Root = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8"
if (Test-Path $cuda128Root) {
    $env:CUDA_PATH = $cuda128Root
    $env:PATH = "$cuda128Root\bin;$env:PATH"
    Write-Host "0/5 已将本次编译临时切换到 CUDA 12.8 ($cuda128Root)" -ForegroundColor Cyan
} else {
    Write-Host "0/5 警告：未找到 $cuda128Root，本次将回退使用系统默认 CUDA_PATH ($env:CUDA_PATH)。" -ForegroundColor Yellow
    Write-Host "        已知 CUDA 13.x 在 Blackwell(sm_120) 上 MMQ kernel 可能崩溃回退到更慢的 cuBLAS，建议单独安装 CUDA 12.8。" -ForegroundColor Yellow
}
$nvccVersion = & nvcc --version 2>&1 | Select-String "release"
Write-Host "        当前 nvcc: $nvccVersion" -ForegroundColor Cyan

# 1. 检查 llama.cpp 源码目录
$sourceDir = "llama.cpp-b10199"
if (-not (Test-Path "llama.cpp-b10199")) {
    $sourceDir = "llama-source"
}
if (-not (Test-Path $sourceDir)) {
    Write-Host "1/5 Cloning llama.cpp source..." -ForegroundColor Cyan
    git clone --depth 50 https://github.com/ggml-org/llama.cpp.git llama-source
    $sourceDir = "llama-source"
}
Write-Host "1/5 Using source directory [$sourceDir]" -ForegroundColor Cyan
Set-Location $sourceDir

# 2. 提取真实构建号
$tagRaw = ""
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest"
    $tagRaw = $release.tag_name
} catch { $tagRaw = "" }
$tagNum = [regex]::Match($tagRaw, '\d+').Value
if (-not $tagNum) { $tagNum = "10199" }
$commitHash = (git rev-parse --short HEAD 2>$null)
if (-not $commitHash) { $commitHash = "b10199" }
Write-Host "2/5 提取版本信息: Build #$tagNum ($commitHash)" -ForegroundColor Cyan

# 3. 自动修补 CMakeLists.txt，静态化 cudart 和 cublasLt（Zero DLL 能做到的部分）
Get-ChildItem -Path . -Recurse -Filter "CMakeLists.txt" | ForEach-Object {
    $content = Get-Content $_.FullName -Raw
    $content = $content -replace 'CUDA::cudart\b', 'CUDA::cudart_static'
    $content = $content -replace 'CUDA::cublasLt\b', 'CUDA::cublasLt_static'
    $content = $content -replace 'CUDA::cublas_static\b', 'CUDA::cublas'
    Set-Content $_.FullName $content
}

# 4. 清理并重建 build 目录以彻底排除上一次配置缓存
if (Test-Path "build") {
    Remove-Item -Path "build" -Recurse -Force
}
New-Item -ItemType Directory -Name "build" | Out-Null
Set-Location build
Write-Host "3/5 正在配置 CMake 参数 (针对 RTX 5060 Ti sm_120 精确加速)..." -ForegroundColor Cyan
cmake .. -DCMAKE_BUILD_TYPE=Release `
  "-DLLAMA_BUILD_NUMBER=$tagNum" `
  "-DLLAMA_BUILD_COMMIT=$commitHash" `
  -DCMAKE_CUDA_RUNTIME_LIBRARY=Static `
  -DBUILD_SHARED_LIBS=OFF `
  -DGGML_CUDA=ON `
  -DGGML_CUDA_FORCE_CUBLAS=OFF `
  -DCMAKE_CUDA_ARCHITECTURES="120" `
  -DGGML_CUDA_FA_ALL_QUANTS=ON `
  -DLLAMA_BUILD_EXAMPLES=OFF `
  -DLLAMA_BUILD_TESTS=OFF `
  -DLLAMA_BUILD_SERVER=ON `
  -DGGML_NATIVE=OFF `
  -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON
if ($LASTEXITCODE -ne 0) { throw "CMake 配置失败，退出码 $LASTEXITCODE" }

# 5. 调用 MSVC 并行编译
Write-Host "4/5 正在进行多核并行编译 (-j 16)..." -ForegroundColor Cyan
cmake --build . --config Release --target llama-server -j 16
if ($LASTEXITCODE -ne 0) { throw "编译失败，退出码 $LASTEXITCODE" }
Set-Location ..\..

# 6. 复制生成的二进制到根目录及 src-tauri/resources，自动补上 cublas64_*.dll
$exe = Get-ChildItem -Path "$sourceDir\build" -Recurse -Filter "llama-server.exe" | Select-Object -First 1
if ($exe) {
    Copy-Item $exe.FullName -Destination ".\llama-server.exe" -Force
    $targetRes = ".\src-tauri\resources"
    if (-not (Test-Path $targetRes)) { New-Item -ItemType Directory -Path $targetRes -Force | Out-Null }
    Copy-Item $exe.FullName -Destination "$targetRes\llama-server.exe" -Force

    $cublasDll = Get-ChildItem -Path "$env:CUDA_PATH\bin" -Filter "cublas64_*.dll" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cublasDll) {
        Copy-Item $cublasDll.FullName -Destination ".\$($cublasDll.Name)" -Force
        Copy-Item $cublasDll.FullName -Destination "$targetRes\$($cublasDll.Name)" -Force
        Write-Host "        已自动拷贝 $($cublasDll.Name) 到根目录和 src-tauri/resources 目录" -ForegroundColor Cyan
    } else {
        Write-Warning "没能在 $env:CUDA_PATH\bin 下找到 cublas64_*.dll，请手动拷贝到 src-tauri/resources 目录"
    }

    Write-Host "5/5 正在校验静态链接是否生效 (dumpbin)..." -ForegroundColor Cyan
    $deps = & dumpbin /DEPENDENTS ".\llama-server.exe" 2>&1
    if ($deps -match 'cudart64|cublasLt64') {
        Write-Host "==========================================" -ForegroundColor Red
        Write-Host "警告：cudart 或 cublasLt 仍然是动态依赖，静态化补丁没生效！" -ForegroundColor Red
        $deps | Select-String -Pattern 'cudart64|cublasLt64' | Write-Host
        Write-Host "==========================================" -ForegroundColor Red
    } else {
        Write-Host "==========================================" -ForegroundColor Green
        Write-Host "恭喜！编译成功，cudart/cublasLt 已确认静态化。" -ForegroundColor Green
        Write-Host "llama-server.exe 和 DLL 已成功注入到 src-tauri/resources/ 资源包目录中。" -ForegroundColor Green
        Write-Host "==========================================" -ForegroundColor Green
    }
} else {
    Write-Error "编译似乎失败了：在 $sourceDir\build 下没有找到 llama-server.exe"
}