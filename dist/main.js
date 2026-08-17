var builtins = {};
var customProfiles = {};
var healthTimer = null;
var tauriInvoke = (window.__TAURI__ && window.__TAURI__.core) ? window.__TAURI__.core.invoke : null;

function $(i) { return document.getElementById(i); }

// --- Tab Switching ---
function switchTab(tabName) {
    document.querySelectorAll('.nav-tab').forEach(function(btn) { btn.classList.remove('active'); });
    document.querySelectorAll('.tab-view').forEach(function(view) { view.classList.remove('active'); });

    if (tabName === 'settings') {
        $('tabBtnSettings').classList.add('active');
        $('viewSettings').classList.add('active');
    } else if (tabName === 'chat') {
        $('tabBtnChat').classList.add('active');
        $('viewChat').classList.add('active');
        setTimeout(fitChatHeight, 50);
    } else if (tabName === 'gateway') {
        $('tabBtnGateway').classList.add('active');
        $('viewGateway').classList.add('active');
    } else if (tabName === 'compile') {
        $('tabBtnCompile').classList.add('active');
        $('viewCompile').classList.add('active');
        refreshBuildEnv();
    }
}

function copyText(txt) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(txt).then(function() {
            showToast('已复制到剪贴板！');
        }).catch(function() {
            showToast('复制失败');
        });
    } else {
        showToast('剪贴板 API 不可用');
    }
}

// --- File browsing ---
async function browseFile(tid, type) {
    var filterName = '', extension = '';
    if (type === 'exe') { filterName = 'EXE程序'; extension = 'exe'; }
    else if (type === 'gguf') { filterName = 'GGUF模型'; extension = 'gguf'; }
    else if (type === 'jinja') { filterName = 'Jinja模板 (*.jinja)'; extension = 'jinja'; }
    else if (type === 'dir') { filterName = '文件夹'; extension = 'dir'; }

    try {
        var file = await tauriInvoke('browse_file', { filterName: filterName, extension: extension });
        if (file) {
            $(tid).value = file;
            if (tid === 'modelPath' || tid === 'draftModelPath') onModelPathChange();
            updatePreview();
        }
    } catch (e) {
        console.error('browse_file error:', e);
    }
}

var lastRecommendedNgl = null;

function onModelPathChange() {
    var path = $('modelPath').value.toLowerCase();
    var draftPath = $('draftModelPath') ? $('draftModelPath').value.toLowerCase() : '';
    var spec = $('specType');
    if (draftPath.indexOf('dflash') !== -1 || path.indexOf('dflash') !== -1) {
        if (spec.value === 'none') spec.value = 'draft-dflash';
    } else if (path.indexOf('mtp') !== -1) {
        if (spec.value === 'none') spec.value = 'draft-mtp';
    } else {
        if (spec.value === 'draft-mtp' || spec.value === 'draft-dflash') spec.value = 'none';
    }
    updateSpecVis();
    updatePreview();
    inspectModelPath();
}

async function inspectModelPath() {
    var path = $('modelPath').value.trim().replace(/^["']|["']$/g, '');
    var panel = $('ggufInspector');
    if (!path || !window.__TAURI__) {
        if (panel) panel.style.display = 'none';
        return;
    }

    try {
        var info = await tauriInvoke('inspect_gguf', { path: path });
        if (!info || !info.architecture) {
            panel.style.display = 'none';
            return;
        }
        panel.style.display = 'block';
        $('ggufArch').textContent = info.architecture.toUpperCase();
        $('ggufName').textContent = info.model_name || path.split(/[\\/]/).pop();
        $('ggufQuant').textContent = info.quantization_type || 'Quantized';

        var sizeGb = (info.file_size_bytes / (1024 * 1024 * 1024)).toFixed(2);
        $('ggufSize').textContent = sizeGb + ' GB';
        if (info.has_mtp && info.block_count > 1) {
            $('ggufLayers').textContent = (info.block_count - 1) + ' + 1 MTP 层 (共 ' + info.block_count + ' 层)';
        } else {
            $('ggufLayers').textContent = (info.block_count || '—') + ' 层';
        }
        $('ggufMaxCtx').textContent = info.context_length ? (info.context_length >= 1024 ? (info.context_length / 1024) + 'k' : info.context_length) : '未知';

        var ctx = parseInt($('ctxSize').value, 10) || 8192;
        var cacheK = $('cacheK').value || 'q8_0';
        var cacheV = $('cacheV').value || 'q8_0';
        var est = await tauriInvoke('estimate_vram_budget', {
            path: path,
            ctxSize: ctx,
            cacheTypeK: cacheK,
            cacheTypeV: cacheV,
            gpuVramGb: 16.0
        });

        lastRecommendedNgl = est.recommended_ngl;

        var vramTxt = est.total_estimated_vram_gb.toFixed(1) + ' GB / 16.0 GB';
        $('ggufVramEst').textContent = vramTxt;
        if (est.can_fully_offload) {
            $('ggufVramEst').style.color = '#4ade80';
            $('ggufNglHint').innerHTML = '⚡ 显存完全包含模型 (' + est.total_estimated_vram_gb.toFixed(1) + 'GB)，推荐 GPU 层数 (-ngl): <b>' + est.recommended_ngl + ' 层 (100% 卸载)</b>';
        } else {
            $('ggufVramEst').style.color = '#f59e0b';
            $('ggufNglHint').innerHTML = '⚠️ 显存紧凑 (预计 ' + est.total_estimated_vram_gb.toFixed(1) + 'GB)，推荐 GPU 卸载: <b>' + est.recommended_ngl + ' / ' + (info.block_count || '—') + ' 层 (' + est.offload_percentage.toFixed(0) + '%)</b>';
        }
    } catch(e) {
        if (panel) panel.style.display = 'none';
    }
}

function applyRecommendedNgl() {
    if (lastRecommendedNgl !== null) {
        $('gpuLayers').value = lastRecommendedNgl;
        updatePreview();
        showToast('已一键应用最佳 GPU 层数 (-ngl): ' + lastRecommendedNgl);
    }
}

// --- Profile management ---
function refreshProfiles(sel) {
    var s = $('profileSelect');
    s.innerHTML = '';
    
    var bKeys = Object.keys(builtins);
    var cKeys = Object.keys(customProfiles);
    var all = bKeys.concat(cKeys.filter(function(k) { return bKeys.indexOf(k) < 0; }));
    
    if (all.length === 0) {
        var o = document.createElement('option');
        o.value = '__default__';
        o.textContent = '默认配置';
        s.appendChild(o);
        return;
    }
    all.forEach(function(n) {
        var o = document.createElement('option');
        o.value = n; o.textContent = n;
        s.appendChild(o);
    });
    if (sel && all.indexOf(sel) >= 0) {
        s.value = sel;
    } else if (all.length > 0) {
        s.value = all[0];
    }
}

function loadProfile() {
    var name = $('profileSelect').value;
    var f = builtins[name] || customProfiles[name];
    if (!f) return;
    applyFields(f);
    updatePreview();
    inspectModelPath();
    localStorage.setItem('lastProfile', name);
}

async function saveProfile() {
    var name = $('profileSelect').value;
    var bKeys = Object.keys(builtins);
    
    if (name === '__default__' || bKeys.indexOf(name) >= 0 || !customProfiles[name]) {
        var defaultNewName = (bKeys.indexOf(name) >= 0) ? name + ' (自定义)' : '我的配置';
        name = prompt(bKeys.indexOf(name) >= 0 ? '内置配置不可覆盖\n输入新配置名称：' : '输入配置名称：', defaultNewName);
        if (!name || !name.trim()) return;
        name = name.trim();
    }
    customProfiles[name] = buildFields();
    if (window.__TAURI__) {
        try { await tauriInvoke('save_profiles', { profiles: customProfiles }); }
        catch (e) { console.error('保存配置失败:', e); }
    }
    refreshProfiles(name);
    localStorage.setItem('lastProfile', name);
}

async function deleteProfile() {
    var name = $('profileSelect').value;
    var bKeys = Object.keys(builtins);
    if (name === '__default__' || bKeys.indexOf(name) >= 0) return;
    if (!confirm('删除配置「' + name + '」？')) return;
    delete customProfiles[name];
    if (window.__TAURI__) {
        try { await tauriInvoke('save_profiles', { profiles: customProfiles }); }
        catch (e) {}
    }
    refreshProfiles();
    loadProfile();
}

// --- Fields build / apply ---
function buildFields() {
    return {
        serverPath: $('serverPath').value,
        modelPath: $('modelPath').value,
        draftModelPath: $('draftModelPath').value,
        mmprojPath: $('mmprojPath').value,
        mmprojEnabled: $('mmprojToggle').checked,
        alias: $('alias').value,
        port: $('port').value,
        host: $('hostToggle').checked ? '0.0.0.0' : '',
        cudaDevice: $('cudaDevice').value,
        gpuLayers: $('gpuLayers').value,
        ctxSize: $('ctxSize').value,
        batchSize: $('batchSize').value,
        ubatchSize: $('ubatchSize').value,
        numPhysGpu: $('numPhysGpu').value,
        cacheK: $('cacheK').value,
        cacheV: $('cacheV').value,
        cacheRam: $('cacheRam').value,
        threads: $('threads').value,
        threadsBatch: $('threadsBatch').value,
        jinja: $('jinja').checked,
        flashAttn: $('flashAttn').checked,
        reasoningPreserve: $('reasoningPreserve') ? $('reasoningPreserve').checked : false,
        kvUnified: $('kvUnified').checked,
        contBatching: $('contBatching').checked,
        metrics: $('metrics').checked,
        reasoning: $('reasoning').checked,
        reasoningBudget: $('reasoningBudget').value,
        specType: $('specType').value,
        draftN: $('draftN').value,
        draftNgl: $('draftNgl').value,
        draftTypeK: $('draftTypeK').value,
        draftTypeV: $('draftTypeV').value,
        logVerbosity: $('logVerbosity').value,
        logFormat: $('logFormat').value,
        imageMinTokens: $('imageMinTokens').value,
        extraParams: $('extraParams').value,
        chatTemplateFile: $('chatTemplateFile') ? $('chatTemplateFile').value : '',
        reasoningEffort: $('reasoningEffort') ? $('reasoningEffort').value : 'default',
        enableAnthropicProxy: $('enableAnthropicProxy').checked,
        anthropicProxyPort: $('anthropicProxyPort').value,
        anthropicApiKey: $('anthropicApiKey').value
    };
}

function applyFields(f) {
    var ht = $('hostToggle');
    if (ht && f.hasOwnProperty('host')) ht.checked = (f.host === '0.0.0.0');
    var mt = $('mmprojToggle');
    if (mt && f.hasOwnProperty('mmprojEnabled')) mt.checked = !!f.mmprojEnabled;
    updateMmprojDim();

    ['serverPath','modelPath','draftModelPath','mmprojPath','alias','port','cudaDevice',
     'gpuLayers','ctxSize','batchSize','ubatchSize','numPhysGpu','cacheRam',
     'threads','threadsBatch','reasoningBudget','draftN','draftNgl',
     'imageMinTokens','extraParams','chatTemplateFile','anthropicProxyPort','anthropicApiKey'].forEach(function(k) {
        var e = $(k); if (e && f.hasOwnProperty(k)) e.value = f[k];
    });

    ['jinja','flashAttn','reasoningPreserve','kvUnified','contBatching','metrics','reasoning','enableAnthropicProxy'].forEach(function(k) {
        var e = $(k); if (e && f.hasOwnProperty(k)) e.checked = !!f[k];
    });

    ['cacheK','cacheV','draftTypeK','draftTypeV','specType','logVerbosity','logFormat','reasoningEffort'].forEach(function(k) {
        var e = $(k); if (e && f.hasOwnProperty(k)) e.value = f[k];
    });

    updateSpecVis();
}

function updateMmprojDim() {
    var on = $('mmprojToggle').checked;
    $('mmprojPath').style.opacity = on ? '1' : '0.35';
    $('mmprojPath').style.pointerEvents = on ? '' : 'none';
    $('mmprojBrowse').disabled = !on;
}

function updateSpecVis() {
    var on = $('specType').value !== 'none';
    ['cDN','cDNgl','cDK','cDV'].forEach(function(id) {
        $(id).style.display = on ? 'flex' : 'none';
    });
}

// --- Build command array ---
function buildCmd() {
    var f = buildFields(), L = [];
    L.push(f.serverPath);
    L.push('-m', f.modelPath);
    if (f.draftModelPath) {
        L.push('--model-draft', f.draftModelPath);
        if (f.specType === 'none' && f.draftN) L.push('--spec-draft-n-max', f.draftN);
    }
    if (f.mmprojEnabled && f.mmprojPath) L.push('--mmproj', f.mmprojPath);
    if (f.alias) L.push('--alias', f.alias);
    L.push('--port', f.port);
    if (f.host) L.push('--host', f.host);
    if (f.cudaDevice !== '' && f.cudaDevice !== undefined && f.cudaDevice !== null) L.push('--main-gpu', f.cudaDevice);
    L.push('--n-gpu-layers', f.gpuLayers);
    L.push('-c', f.ctxSize);
    if (f.batchSize) L.push('-b', f.batchSize);
    if (f.ubatchSize) L.push('-ub', f.ubatchSize);
    if (f.numPhysGpu) L.push('-np', f.numPhysGpu);
    L.push('--cache-type-k', f.cacheK, '--cache-type-v', f.cacheV);
    if (f.cacheRam) L.push('--cache-ram', f.cacheRam);
    if (f.threads) L.push('-t', f.threads);
    if (f.threadsBatch) L.push('-tb', f.threadsBatch);
    if (f.chatTemplateFile) {
        L.push('--jinja');
        L.push('--chat-template-file', f.chatTemplateFile);
    } else if (f.jinja) {
        L.push('--jinja');
    }
    if (f.flashAttn) L.push('--flash-attn', 'on');
    if (f.reasoningPreserve) L.push('--reasoning-preserve');
    L.push(f.kvUnified ? '--kv-unified' : '--no-kv-unified');
    if (f.contBatching) L.push('--cont-batching');
    if (f.metrics) L.push('--metrics');
    L.push('--reasoning', f.reasoning ? 'on' : 'off');
    if (f.reasoning && f.reasoningBudget) L.push('--reasoning-budget', f.reasoningBudget);
    if (f.specType !== 'none') {
        L.push('--spec-type', f.specType);
        if (f.draftN) L.push('--spec-draft-n-max', f.draftN);
        if (f.draftNgl) L.push('--spec-draft-ngl', f.draftNgl);
        L.push('--spec-draft-type-k', f.draftTypeK, '--spec-draft-type-v', f.draftTypeV);
    }
    if (f.logVerbosity && f.logVerbosity !== '0') L.push('--log-verbosity', f.logVerbosity);
    if (f.logFormat && f.logFormat !== 'text') L.push('--log-format', f.logFormat);
    if (f.imageMinTokens) L.push('--image-min-tokens', f.imageMinTokens);
    f.extraParams.replace(/\r\n/g, '\n').split('\n').forEach(function(p) {
        p = p.trim();
        if (p) {
            if (!/^--/.test(p)) { alert('⚠️ 跳过无效额外参数（必须以 -- 开头）: ' + p); return; }
            var parts = p.split(/\s+/);
            parts.forEach(function(part) {
                L.push(part);
            });
        }
    });
    return L;
}

function esc(s) {
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function updateActiveServerStatus() {
    var serverPath = $('serverPath') ? $('serverPath').value.trim() : '';
    var card = $('envActiveServerCard');
    var valEl = $('envActiveServerVal');
    if (!valEl) return;

    if (serverPath) {
        var fileName = serverPath.split(/[\\/]/).pop();
        valEl.textContent = fileName || serverPath;
        valEl.title = '【点击向控制台输出 --version 版本】当前指定运行内核: ' + serverPath;
        if (card) card.className = 'env-card ok';
    } else {
        valEl.textContent = '默认同级内核 (llama-server.exe)';
        valEl.title = '【点击向控制台输出 --version 版本】未显式指定路径，主服务将默认运行同级/内置 llama-server.exe';
        if (card) card.className = 'env-card ok';
    }
}

async function checkActiveServerVersion() {
    if (!tauriInvoke) return;
    var serverPath = $('serverPath') ? $('serverPath').value.trim() : '';
    appendTermLine('🔍 正在检测目标内核版本信息 (llama-server.exe --version)...', 'cyan');
    try {
        var ver = await tauriInvoke('get_server_version', { serverPath: serverPath });
        appendTermLine('========================================', 'green');
        appendTermLine('📋 运行内核版本详情 (' + (serverPath || '默认同级 llama-server.exe') + '):', 'green');
        ver.split(/\r?\n/).forEach(function(line) {
            if (line.trim()) appendTermLine('   ' + line.trim(), 'info');
        });
        appendTermLine('========================================', 'green');
    } catch(e) {
        appendTermLine('❌ 获取内核版本失败: ' + e, 'err');
    }
}

function updatePreview() {
    var cmd = buildCmd(), parts = [esc(cmd[0])], i = 1;
    while (i < cmd.length) {
        var t = cmd[i];
        if (t && t.charAt(0) === '-' && i + 1 < cmd.length && cmd[i+1] && cmd[i+1].charAt(0) !== '-') {
            parts.push('<span class="fg">' + esc(t) + '</span> <span class="fv">' + esc(cmd[i+1]) + '</span>');
            i += 2;
        } else if (t) {
            parts.push('<span class="fg">' + esc(t) + '</span>');
            i++;
        } else { i++; }
    }
    $('cmdPreview').innerHTML = parts.join(' \\\n    ');
    updateActiveServerStatus();
}

// --- Status bar ---
function setStatus(cls, txt) {
    $('status').className = 'st' + (cls ? ' ' + cls : '');
    $('stText').textContent = txt;
}

// --- Server start/stop ---
async function startServer() {
    var f = buildFields();
    if (!f.serverPath) { alert('❌ 请先设置服务器路径'); return; }
    if (!f.modelPath) { alert('❌ 请先设置模型路径'); return; }

    var args = buildCmd().filter(function(a) { return a !== ''; });

    $('btnStart').disabled = true;
    $('btnStop').disabled = false;
    setStatus('r', '🚀 启动中，正在加载模型...');

    if (healthTimer) { clearInterval(healthTimer); healthTimer = null; }

    if (window.__TAURI__) {
        try {
            await tauriInvoke('start_server', {
                args: args,
                cudaDevice: f.cudaDevice,
                enableAnthropicProxy: f.enableAnthropicProxy,
                anthropicProxyPort: parseInt(f.anthropicProxyPort, 10) || 8081,
                anthropicApiKey: f.anthropicApiKey || ''
            });
            healthTimer = setInterval(checkHealth, 2000);
        } catch (e) {
            setStatus('e', '启动失败: ' + e);
            $('btnStart').disabled = false;
            $('btnStop').disabled = true;
        }
    } else {
        console.log('[Dev] args:', args.join(' '));
        healthTimer = setInterval(checkHealth, 2000);
    }
}

async function stopServer() {
    if (healthTimer) { clearInterval(healthTimer); healthTimer = null; }
    if (window.__TAURI__) {
        try { await tauriInvoke('stop_server'); } catch (e) {}
    }
    $('btnStop').disabled = true;
    $('btnStart').disabled = false;
    setStatus('', '已停止');
    setTimeout(function() { setStatus('', '就绪'); }, 1500);
}

function proxyStatusSuffix() {
    if (!$('enableAnthropicProxy').checked) return '';
    var port = $('anthropicProxyPort').value || '8081';
    return ' · Anthropic API: http://localhost:' + port + '/v1/messages';
}

function getActiveAlias() {
    var alias = $('alias').value.trim();
    if (alias) return alias;
    var path = $('modelPath').value.trim();
    if (path) {
        var parts = path.split(/[\\/]/);
        var filename = parts[parts.length - 1];
        return filename.replace(/\.[^/.]+$/, "");
    }
    return 'local-model';
}

function checkHealth() {
    var port = $('port').value || '8080';
    var xhr = new XMLHttpRequest();
    xhr.open('GET', 'http://localhost:' + port + '/health', true);
    xhr.onreadystatechange = function() {
        if (xhr.readyState === 4 && xhr.status === 200) {
            try {
                var res = JSON.parse(xhr.responseText);
                if (res.status === 'ok') {
                    var aliasName = getActiveAlias();
                    setStatus('r', '✅ 模型<' + aliasName + '>成功加载，服务正在运行 · 点击查看日志' + proxyStatusSuffix());
                    clearInterval(healthTimer); healthTimer = null;
                    $('btnStart').disabled = false;
                } else if (res.status === 'loading') {
                    var prog = res.progress ? ' (' + (res.progress * 100).toFixed(1) + '%)' : '';
                    setStatus('r', '⏳ 模型加载中' + prog + '...');
                }
            } catch (e) {}
        }
    };
    xhr.onerror = function() {};
    xhr.send();
}

function checkStartupHealth() {
    var port = $('port').value || '8080';
    var xhr = new XMLHttpRequest();
    xhr.open('GET', 'http://localhost:' + port + '/health', true);
    xhr.onreadystatechange = function() {
        if (xhr.readyState === 4 && xhr.status === 200) {
            $('btnStart').disabled = false;
            $('btnStop').disabled = false;
            var aliasName = getActiveAlias();
            setStatus('r', '✅ 模型<' + aliasName + '>成功加载，服务正在运行 · 点击查看日志');
        }
    };
    xhr.onerror = function() {};
    xhr.send();
}

async function openLog() {
    if (window.__TAURI__) {
        try { await tauriInvoke('open_log'); }
        catch (e) { console.error('打开日志失败:', e); }
    }
}

function resetDefaults() {
    var bKeys = Object.keys(builtins);
    if (bKeys.length > 0) {
        $('profileSelect').value = bKeys[0];
        loadProfile();
    } else {
        var cKeys = Object.keys(customProfiles);
        if (cKeys.length > 0) {
            $('profileSelect').value = cKeys[0];
            loadProfile();
        }
    }
}

// --- Chat ---
var chatHist = [];
var chatBusy = false;
var chatOpen = true;
var chatTokCount = 0, chatT0 = 0;

function toggleChat() {
    chatOpen = !chatOpen;
    $('chatBody').style.display = chatOpen ? '' : 'none';
    $('chatToggleIcon').textContent = chatOpen ? '▲' : '▼';
    fitChatHeight();
}

function clearChat() {
    chatHist = [];
    $('chatMsgs').innerHTML = '';
}

function appendBubble(cls, txt) {
    var d = document.createElement('div');
    d.className = cls;
    if (txt) d.textContent = txt;
    $('chatMsgs').appendChild(d);
    $('chatMsgs').scrollTop = $('chatMsgs').scrollHeight;
    return d;
}

function sendChat() {
    if (chatBusy) return;
    var txt = $('chatInput').value.trim();
    if (!txt) return;
    $('chatInput').value = '';
    chatHist.push({ role: 'user', content: txt });
    appendBubble('msg-u', txt);

    var msgs = [];
    var sys = $('chatSys').value.trim();
    if (sys) msgs.push({ role: 'system', content: sys });
    msgs = msgs.concat(chatHist);

    var aiDiv = appendBubble('msg-a', '▌');
    var buf = '', lastIdx = 0;
    chatTokCount = 0; chatT0 = 0;
    $('chatSpeed').style.color = '#64748b';
    $('chatSpeed').textContent = '— t/s';

    chatBusy = true;
    $('btnSend').disabled = true;
    $('btnSend').textContent = '…';

    var xhr = new XMLHttpRequest();
    var port = $('port').value || '8080';
    xhr.open('POST', 'http://localhost:' + port + '/v1/chat/completions', true);
    xhr.setRequestHeader('Content-Type', 'application/json');

    xhr.onreadystatechange = function() {
        if (xhr.readyState >= 3) {
            var chunk = xhr.responseText.substring(lastIdx);
            lastIdx = xhr.responseText.length;
            chunk.split('\n').forEach(function(line) {
                line = line.trim();
                if (line.indexOf('data: ') === 0) {
                    var raw = line.substring(6);
                    if (raw === '[DONE]') return;
                    try {
                        var obj = JSON.parse(raw);
                        var delta = obj.choices && obj.choices[0] && obj.choices[0].delta && obj.choices[0].delta.content;
                        if (delta) {
                            if (!chatT0) chatT0 = Date.now();
                            chatTokCount++;
                            buf += delta;
                            aiDiv.textContent = buf + '▌';
                            $('chatMsgs').scrollTop = $('chatMsgs').scrollHeight;
                            var elapsed = (Date.now() - chatT0) / 1000;
                            if (elapsed > 0.1) {
                                $('chatSpeed').style.color = '#4ade80';
                                $('chatSpeed').textContent = (chatTokCount / elapsed).toFixed(1) + ' t/s';
                            }
                        }
                    } catch (e) {}
                }
            });
        }
        if (xhr.readyState === 4) {
            chatBusy = false;
            $('btnSend').disabled = false;
            $('btnSend').textContent = '发送';
            if (buf) {
                aiDiv.innerHTML = mdToHtml(buf);
                chatHist.push({ role: 'assistant', content: buf });
                $('chatSpeed').style.color = '#475569';
            }
            if (xhr.status !== 200 && !buf) {
                aiDiv.className = 'msg-e';
                aiDiv.textContent = xhr.status === 0
                    ? '❌ 连接拒绝：请确认服务已启动且模型加载完毕'
                    : '❌ HTTP ' + xhr.status;
            }
        }
    };

    xhr.onerror = function() {
        chatBusy = false;
        $('btnSend').disabled = false;
        $('btnSend').textContent = '发送';
        aiDiv.className = 'msg-e';
        aiDiv.textContent = '❌ 无法连接到服务，请检查端口 ' + port + ' 是否正确';
    };

    xhr.send(JSON.stringify({
        model: $('alias').value || 'local-model',
        messages: msgs,
        temperature: parseFloat($('chatTemp').value) || 0.7,
        max_tokens: parseInt($('chatMaxTok').value) || 4096,
        stream: true
    }));
}

function mdToHtml(text) {
    if (!text) return '';
    
    // Escape HTML special characters but preserve tags we might generate, so do it first
    var s = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    
    // Restore the escaped thought tags since we want to style them
    s = s.replace(/&lt;thought&gt;/gi, '<thought>').replace(/&lt;\/thought&gt;/gi, '</thought>');
    s = s.replace(/&lt;thinking&gt;/gi, '<thinking>').replace(/&lt;\/thinking&gt;/gi, '</thinking>');
    
    // Parse thinking blocks
    // Replace <thought>...</thought> with a styled box
    s = s.replace(/<thought>([\s\S]*?)<\/thought>/gi, function(match, content) {
        return '<div class="thought-block"><div class="thought-hdr">🧠 思考过程</div><div class="thought-body">' + content.trim() + '</div></div>';
    });
    s = s.replace(/<thinking>([\s\S]*?)<\/thinking>/gi, function(match, content) {
        return '<div class="thought-block"><div class="thought-hdr">🧠 思考过程</div><div class="thought-body">' + content.trim() + '</div></div>';
    });

    // Parse multi-line code blocks: ```lang ... ```
    var codeBlockRegex = /```([a-zA-Z0-9_\-+]*)\n([\s\S]*?)```/g;
    s = s.replace(codeBlockRegex, function(match, lang, code) {
        var cleanLang = lang ? lang.trim() : 'text';
        var cleanCode = code.replace(/^\n/, '').replace(/\n$/, '');
        var uniqId = 'code-' + Math.random().toString(36).substr(2, 9);
        return '<div class="code-wrapper">' +
               '  <div class="code-header">' +
               '    <span class="code-lang">' + cleanLang + '</span>' +
               '    <button class="code-copy-btn" onclick="copyCode(\'' + uniqId + '\')">📋 复制</button>' +
               '  </div>' +
               '  <pre><code id="' + uniqId + '">' + cleanCode + '</code></pre>' +
               '</div>';
    });

    // Process line-by-line for blocks
    var lines = s.split('\n');
    var html = [];
    var inList = false;
    var listType = ''; // 'ul' or 'ol'
    
    for (var i = 0; i < lines.length; i++) {
        var line = lines[i];
        
        // Skip code-wrappers or thought blocks which already have tags
        if (line.indexOf('<div class="code-wrapper"') !== -1 || 
            line.indexOf('<div class="thought-block"') !== -1 ||
            line.indexOf('<pre>') !== -1 || 
            line.indexOf('</pre>') !== -1 || 
            line.indexOf('<code>') !== -1 || 
            line.indexOf('</code>') !== -1 ||
            line.indexOf('</div') !== -1 ||
            line.indexOf('<div class="thought-hdr"') !== -1 ||
            line.indexOf('<div class="thought-body"') !== -1) {
            
            if (inList) {
                html.push('</' + listType + '>');
                inList = false;
            }
            html.push(line);
            continue;
        }

        // Headers
        var headerMatch = line.match(/^(#{1,6})\s+(.+)$/);
        if (headerMatch) {
            if (inList) {
                html.push('</' + listType + '>');
                inList = false;
            }
            var level = headerMatch[1].length;
            html.push('<h' + level + '>' + headerMatch[2] + '</h' + level + '>');
            continue;
        }

        // Unordered List item
        var ulMatch = line.match(/^[\*\-]\s+(.+)$/);
        if (ulMatch) {
            if (!inList || listType !== 'ul') {
                if (inList) html.push('</' + listType + '>');
                html.push('<ul>');
                inList = true;
                listType = 'ul';
            }
            html.push('<li>' + parseInline(ulMatch[1]) + '</li>');
            continue;
        }

        // Ordered List item
        var olMatch = line.match(/^(\d+)\.\s+(.+)$/);
        if (olMatch) {
            if (!inList || listType !== 'ol') {
                if (inList) html.push('</' + listType + '>');
                html.push('<ol>');
                inList = true;
                listType = 'ol';
            }
            html.push('<li>' + parseInline(olMatch[2]) + '</li>');
            continue;
        }

        // Blank line closes lists
        if (line.trim() === '') {
            if (inList) {
                html.push('</' + listType + '>');
                inList = false;
            }
            html.push('<br>');
            continue;
        }

        // Normal paragraph
        if (inList) {
            html.push('</' + listType + '>');
            inList = false;
        }
        html.push('<p>' + parseInline(line) + '</p>');
    }

    if (inList) {
        html.push('</' + listType + '>');
    }

    return html.join('\n');

    // Inner helper to parse bold, italic, inline code
    function parseInline(txt) {
        var t = txt;
        // Bold: **text**
        t = t.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
        // Italic: *text*
        t = t.replace(/\*([^*\n]+)\*/g, '<em>$1</em>');
        // Inline code: `code`
        t = t.replace(/`([^`\n]+)`/g, '<code>$1</code>');
        return t;
    }
}

window.copyCode = function(uniqId) {
    var codeEl = document.getElementById(uniqId);
    if (!codeEl) return;
    var text = codeEl.textContent;
    
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).then(function() {
            showToast('已复制代码到剪贴板');
        }).catch(function(e) {
            console.error('Failed to copy code: ', e);
        });
    } else {
        var textarea = document.createElement('textarea');
        textarea.value = text;
        document.body.appendChild(textarea);
        textarea.select();
        try {
            document.execCommand('copy');
            showToast('已复制代码到剪贴板');
        } catch (err) {
            console.error('Fallback copy failed', err);
        }
        document.body.removeChild(textarea);
    }
};

function showToast(msg) {
    var toast = document.createElement('div');
    toast.className = 'toast-notification';
    toast.textContent = msg;
    document.body.appendChild(toast);
    
    setTimeout(function() {
        toast.classList.add('show');
    }, 10);
    
    setTimeout(function() {
        toast.classList.remove('show');
        setTimeout(function() {
            document.body.removeChild(toast);
        }, 300);
    }, 2000);
}

function fitChatHeight() {
    if (!chatOpen) return;
    var msgs = $('chatMsgs');
    if (!msgs) return;
    msgs.style.height = '0px';
    var winH = window.innerHeight || document.documentElement.clientHeight;
    var msgTop = msgs.getBoundingClientRect().top;
    var foot = document.querySelector('.chat-foot');
    var footH = foot ? foot.offsetHeight : 0;
    var newH = Math.max(150, winH - msgTop - 12 - footH - 20 - 24 - 10);
    msgs.style.height = newH + 'px';
}

// --- Init ---
async function init() {
    // 启动时根据屏幕真实可用高度按 90% 动态计算窗口高度（如 2560x1440 屏幕下为 ~1296px），并居中定位
    if (window.__TAURI__ && window.__TAURI__.window) {
        try {
            var W = window.__TAURI__.window;
            var win = W.getCurrentWindow();
            var current = await win.outerSize();
            var scale = await win.scaleFactor();
            var w = Math.round(current.width / scale);
            var h = Math.max(760, Math.round(window.screen.availHeight * 0.90));
            var x = Math.round((window.screen.availWidth - w) / 2);
            var y = Math.max(0, Math.round((window.screen.availHeight - h) / 2));
            await win.setSize(new W.LogicalSize(w, h));
            await win.setPosition(new W.LogicalPosition(x, y));
        } catch(e) { console.error('窗口大小调整失败:', e); }
    }

    if (window.__TAURI__) {
        try { builtins = await tauriInvoke('load_builtins'); }
        catch (e) { console.error('加载内置配置失败:', e); builtins = {}; }

        try { customProfiles = await tauriInvoke('load_profiles'); }
        catch (e) { customProfiles = {}; }
    }

    refreshProfiles(localStorage.getItem('lastProfile'));
    loadProfile();

    if (window.__TAURI__) {
        try {
            var bundledPath = await tauriInvoke('get_bundled_server_path');
            if (bundledPath) {
                var currentPath = $('serverPath').value;
                if (!currentPath || currentPath === 'C:\\AI\\llama\\llama-server.exe') {
                    $('serverPath').value = bundledPath;
                    updatePreview();
                }
            }
        } catch(e) {}
    }
    updatePreview();
    updateMmprojDim();
    updateSpecVis();
    checkStartupHealth();
    fitChatHeight();

    // 延时 100ms 确保页面节点与路径填充完毕后自动感知模型并显示 GGUF 卡片与 NGL 推荐
    setTimeout(function() {
        inspectModelPath();
    }, 100);

    ['ctxSize','cacheK','cacheV'].forEach(function(id) {
        var el = $(id);
        if (el) {
            el.addEventListener('input', inspectModelPath);
            el.addEventListener('change', inspectModelPath);
        }
    });

    var draftEl = $('draftModelPath');
    if (draftEl) {
        draftEl.addEventListener('input', onModelPathChange);
        draftEl.addEventListener('change', onModelPathChange);
    }

    document.querySelectorAll('input,select,textarea').forEach(function(el) {
        el.addEventListener('input', updatePreview);
        el.addEventListener('change', updatePreview);
    });

    $('chatInput').addEventListener('keydown', function(e) {
        if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendChat(); }
    });

    window.onresize = fitChatHeight;
}

// --- Engine Compilation Module ---
async function refreshBuildEnv() {
    if (!tauriInvoke) return;
    try {
        var env = await tauriInvoke('check_build_env');
        
        var cudaCard = $('envCudaCard');
        if (env.cuda_installed) {
            cudaCard.className = 'env-card ok';
            $('envCudaVal').textContent = env.cuda_version;
        } else {
            cudaCard.className = 'env-card err';
            $('envCudaVal').textContent = '未找到 CUDA';
        }

        var msvcCard = $('envMsvcCard');
        if (env.msvc_installed) {
            msvcCard.className = 'env-card ok';
            $('envMsvcVal').textContent = 'VS2022 / MSVC 就绪';
        } else {
            msvcCard.className = 'env-card warn';
            $('envMsvcVal').textContent = '未找到 VS 环境';
        }

        var ccacheCard = $('envCcacheCard');
        if (env.ccache_installed) {
            ccacheCard.className = 'env-card ok';
            $('envCcacheVal').textContent = '已就绪 (高速缓存)';
        } else {
            ccacheCard.className = 'env-card info';
            $('envCcacheVal').textContent = '未安装 (常规编译)';
        }

        var ninjaCard = $('envNinjaCard');
        if (env.ninja_installed) {
            ninjaCard.className = 'env-card ok';
            $('envNinjaVal').textContent = 'Ninja 构建就绪';
        } else {
            ninjaCard.className = 'env-card ok';
            $('envNinjaVal').textContent = 'MSBuild 原生构建';
        }

        updateActiveServerStatus();
    } catch(e) {
        console.error('刷新环境失败:', e);
    }
}

async function startCompileEngine(fetchLatest) {
    if (!tauriInvoke) {
        showToast('Tauri IPC 接口未就绪');
        return;
    }

    var btnStart = $('btnStartCompile');
    var btnFetch = $('btnFetchCompile');
    var btnCancel = $('btnCancelCompile');

    btnStart.disabled = true;
    if (btnFetch) btnFetch.disabled = true;
    btnCancel.disabled = false;

    clearBuildTerminal();

    var sourceDir = $('buildSourceDir').value.trim();
    var buildNumber = $('buildNumber').value.trim();

    if (fetchLatest) {
        appendTermLine('⚡ 正在从 GitHub (https://github.com/ggml-org/llama.cpp) 获取官方最新 Release 版本号...', 'cyan');
        try {
            var tag = await tauriInvoke('get_latest_llama_tag');
            if (tag) {
                appendTermLine('✓ 成功获取官方最新版本标签: ' + tag, 'green');
                var m = tag.match(/\d+/);
                var num = m ? m[0] : tag;
                sourceDir = 'C:\\AI\\myllama\\llama.cpp-b' + num;
                buildNumber = 'b' + num;
                $('buildSourceDir').value = sourceDir;
                $('buildNumber').value = buildNumber;
                appendTermLine('📂 已锁定最新源码保存目录: ' + sourceDir, 'info');
            }
        } catch(e) {
            appendTermLine('⚠️ 获取最新版本号失败 (' + e + ')，将尝试下载/更新至 C:\\AI\\myllama\\llama-source', 'warn');
            if (!sourceDir || sourceDir === 'llama-source') {
                sourceDir = 'C:\\AI\\myllama\\llama-source';
                $('buildSourceDir').value = sourceDir;
            }
        }
    }

    var cudaVer = $('buildCudaVer').value;
    var cudaArch = $('buildCudaArch').value;
    var cpuArch = $('buildCpuArch').value;
    var threads = parseInt($('buildThreads').value, 10) || 16;

    appendTermLine('🚀 正在初始化极速编译引擎任务...', 'cyan');
    appendTermLine('源码目录: ' + (sourceDir || 'llama-source') + ' | CUDA: ' + cudaVer + ' | CUDA Arch: sm_' + cudaArch + ' | CPU Arch: ' + cpuArch + ' | 线程数: ' + threads, 'info');

    try {
        var msg = await tauriInvoke('start_compile_engine', {
            sourceDir: sourceDir,
            buildNumber: buildNumber,
            cudaVer: cudaVer,
            cudaArch: cudaArch,
            cpuArch: cpuArch,
            threads: threads
        });
        appendTermLine(msg, 'green');
    } catch(e) {
        appendTermLine('❌ 启动编译失败: ' + e, 'err');
        btnStart.disabled = false;
        if (btnFetch) btnFetch.disabled = false;
        btnCancel.disabled = true;
    }
}

async function cancelCompileEngine() {
    if (!tauriInvoke) return;
    try {
        var msg = await tauriInvoke('cancel_compile_engine');
        appendTermLine('⏹️ ' + msg, 'warn');
        $('btnStartCompile').disabled = false;
        if ($('btnFetchCompile')) $('btnFetchCompile').disabled = false;
        $('btnCancelCompile').disabled = true;
    } catch(e) {
        appendTermLine('取消编译出错: ' + e, 'err');
    }
}

function clearBuildTerminal() {
    $('buildTerminal').innerHTML = '';
}

function appendTermLine(txt, cls) {
    var term = $('buildTerminal');
    if (!term) return;
    var div = document.createElement('div');
    div.className = 'term-line ' + (cls || '');
    
    if (!cls) {
        if (txt.indexOf('0/5') >= 0 || txt.indexOf('1/5') >= 0 || txt.indexOf('2/5') >= 0 || txt.indexOf('3/5') >= 0 || txt.indexOf('4/5') >= 0 || txt.indexOf('5/5') >= 0) {
            div.className = 'term-line cyan';
        } else if (txt.indexOf('恭喜') >= 0 || txt.indexOf('成功') >= 0 || txt.indexOf('100%') >= 0) {
            div.className = 'term-line green';
        } else if (txt.indexOf('警告') >= 0 || txt.indexOf('WARNING') >= 0 || txt.indexOf('[STDERR]') >= 0) {
            div.className = 'term-line warn';
        } else if (txt.indexOf('失败') >= 0 || txt.indexOf('错误') >= 0 || txt.indexOf('Error') >= 0 || txt.indexOf('ERROR') >= 0) {
            div.className = 'term-line err';
        } else {
            div.className = 'term-line info';
        }
    }
    div.textContent = txt;
    term.appendChild(div);
    term.scrollTop = term.scrollHeight;
}

if (window.__TAURI__ && window.__TAURI__.event) {
    window.__TAURI__.event.listen('compile-log', function(ev) {
        appendTermLine(ev.payload);
    });

    window.__TAURI__.event.listen('compile-finished', function(ev) {
        $('btnStartCompile').disabled = false;
        if ($('btnFetchCompile')) $('btnFetchCompile').disabled = false;
        $('btnCancelCompile').disabled = true;

        if (ev.payload && ev.payload.success) {
            appendTermLine('========================================', 'green');
            appendTermLine('🎉 编译 100% 成功！全新的 llama-server.exe 内核已生成！', 'green');
            appendTermLine('========================================', 'green');
            showToast('🎉 编译成功！新内核已就绪');
        } else {
            appendTermLine('❌ 编译未能成功完成 (退出码: ' + (ev.payload ? ev.payload.code : '未知') + ')', 'err');
            showToast('编译未完成，请查看控制台日志');
        }
    });
}

init();
