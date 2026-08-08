use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Serialize, Clone, Default)]
pub struct GgufInfo {
    pub magic: String,
    pub version: u32,
    pub architecture: String,
    pub model_name: String,
    pub block_count: u32,
    pub has_mtp: bool,
    pub context_length: u32,
    pub embedding_length: u32,
    pub head_count: u32,
    pub head_count_kv: u32,
    pub quantization_type: String,
    pub file_size_bytes: u64,
    pub tensor_count: u64,
    pub metadata_count: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct VramEstimate {
    pub total_gpu_vram_gb: f64,
    pub model_vram_gb: f64,
    pub kv_cache_vram_gb: f64,
    pub total_estimated_vram_gb: f64,
    pub recommended_ngl: u32,
    pub can_fully_offload: bool,
    pub offload_percentage: f64,
}

fn parse_string<R: Read>(reader: &mut R) -> std::io::Result<String> {
    let mut len_buf = [0u8; 8];
    reader.read_exact(&mut len_buf)?;
    let len = u64::from_le_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "String length too large",
        ));
    }
    let mut str_buf = vec![0u8; len];
    reader.read_exact(&mut str_buf)?;
    Ok(String::from_utf8_lossy(&str_buf).to_string())
}

fn skip_value<R: Read>(reader: &mut R, val_type: u32) -> std::io::Result<()> {
    match val_type {
        0 | 1 | 7 => {
            let mut b = [0u8; 1];
            reader.read_exact(&mut b)?;
        }
        2 | 3 => {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b)?;
        }
        4 | 5 | 6 => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b)?;
        }
        10 | 11 | 12 => {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b)?;
        }
        8 => {
            let _ = parse_string(reader)?;
        }
        9 => {
            let mut elem_type_buf = [0u8; 4];
            reader.read_exact(&mut elem_type_buf)?;
            let elem_type = u32::from_le_bytes(elem_type_buf);

            let mut len_buf = [0u8; 8];
            reader.read_exact(&mut len_buf)?;
            let array_len = u64::from_le_bytes(len_buf);

            for _ in 0..array_len {
                skip_value(reader, elem_type)?;
            }
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown GGUF value type: {}", val_type),
            ));
        }
    }
    Ok(())
}

fn read_u32<R: Read>(reader: &mut R) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    reader.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(reader: &mut R) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    reader.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn parse_file_type(ft: u32) -> &'static str {
    match ft {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        _ => "Quantized",
    }
}

pub fn inspect_gguf_file<P: AsRef<Path>>(path: P) -> std::io::Result<GgufInfo> {
    let file = File::open(&path)?;
    let file_size_bytes = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(64 * 1024, file);

    let mut magic_buf = [0u8; 4];
    reader.read_exact(&mut magic_buf)?;
    if &magic_buf != b"GGUF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a valid GGUF file (magic header mismatch)",
        ));
    }

    let version = read_u32(&mut reader)?;
    let tensor_count = read_u64(&mut reader)?;
    let metadata_count = read_u64(&mut reader)?;

    let mut info = GgufInfo {
        magic: "GGUF".to_string(),
        version,
        file_size_bytes,
        tensor_count,
        metadata_count,
        ..Default::default()
    };

    let mut file_type_code: Option<u32> = None;

    for _ in 0..metadata_count {
        let key = match parse_string(&mut reader) {
            Ok(k) => k,
            Err(_) => break,
        };

        let val_type = match read_u32(&mut reader) {
            Ok(vt) => vt,
            Err(_) => break,
        };

        match key.as_str() {
            "general.architecture" => {
                if val_type == 8 {
                    if let Ok(arch) = parse_string(&mut reader) {
                        info.architecture = arch;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            "general.name" => {
                if val_type == 8 {
                    if let Ok(name) = parse_string(&mut reader) {
                        info.model_name = name;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            "general.file_type" => {
                if val_type == 4 {
                    if let Ok(ft) = read_u32(&mut reader) {
                        file_type_code = Some(ft);
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            k if k.ends_with(".block_count") || k == "general.block_count" => {
                if val_type == 4 {
                    if let Ok(bc) = read_u32(&mut reader) {
                        info.block_count = bc;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            k if k.ends_with(".context_length") || k == "general.context_length" => {
                if val_type == 4 {
                    if let Ok(cl) = read_u32(&mut reader) {
                        info.context_length = cl;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            k if k.ends_with(".embedding_length") || k == "general.embedding_length" => {
                if val_type == 4 {
                    if let Ok(el) = read_u32(&mut reader) {
                        info.embedding_length = el;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            k if k.ends_with(".attention.head_count") => {
                if val_type == 4 {
                    if let Ok(hc) = read_u32(&mut reader) {
                        info.head_count = hc;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            k if k.ends_with(".attention.head_count_kv") => {
                if val_type == 4 {
                    if let Ok(hckv) = read_u32(&mut reader) {
                        info.head_count_kv = hckv;
                    }
                } else {
                    let _ = skip_value(&mut reader, val_type);
                }
            }
            _ => {
                let _ = skip_value(&mut reader, val_type);
            }
        }
    }

    let path_upper = path.as_ref().to_string_lossy().to_uppercase();
    if path_upper.contains("MTP") || info.model_name.to_uppercase().contains("MTP") {
        info.has_mtp = true;
    }

    if let Some(code) = file_type_code {
        info.quantization_type = parse_file_type(code).to_string();
    } else {
        // Infer from filename if file_type code was not in metadata
        if path_upper.contains("Q4_K_M") {
            info.quantization_type = "Q4_K_M".to_string();
        } else if path_upper.contains("IQ4_XS") {
            info.quantization_type = "IQ4_XS".to_string();
        } else if path_upper.contains("IQ4_NL") {
            info.quantization_type = "IQ4_NL".to_string();
        } else if path_upper.contains("Q8_0") {
            info.quantization_type = "Q8_0".to_string();
        } else if path_upper.contains("Q5_K_M") {
            info.quantization_type = "Q5_K_M".to_string();
        } else if path_upper.contains("F16") || path_upper.contains("BF16") {
            info.quantization_type = "F16".to_string();
        } else {
            info.quantization_type = "Quantized".to_string();
        }
    }

    Ok(info)
}

pub fn estimate_vram(
    info: &GgufInfo,
    selected_ctx: u32,
    cache_type_k: &str,
    cache_type_v: &str,
    total_gpu_vram_gb: f64,
) -> VramEstimate {
    let file_gb = info.file_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

    // Reserved VRAM for OS DWM & Display (~0.8 GB), targeting ~15.2 GB usable VRAM on 16GB GPUs
    let os_cuda_reserve_gb = 0.8;
    let usable_gpu_vram = (total_gpu_vram_gb - os_cuda_reserve_gb).max(1.0);

    let layers = if info.block_count > 0 { info.block_count } else { 32 };
    let embd = if info.embedding_length > 0 { info.embedding_length } else { 4096 };
    let heads = if info.head_count > 0 { info.head_count } else { 32 };
    let heads_kv = if info.head_count_kv > 0 { info.head_count_kv } else { heads };
    let head_dim = if heads > 0 { embd / heads } else { 128 };

    let ctx = if selected_ctx > 0 { selected_ctx } else { 8192 };

    // KV Cache estimation per element bytes
    let bytes_per_elem_k = match cache_type_k {
        "q4_0" => 0.55,
        "q8_0" => 1.05,
        "f16" => 2.0,
        _ => 1.05,
    };
    let bytes_per_elem_v = match cache_type_v {
        "q4_0" => 0.55,
        "q8_0" => 1.05,
        "f16" => 2.0,
        _ => 1.05,
    };

    // KV Cache size = layers * heads_kv * head_dim * ctx * (k_bytes + v_bytes)
    let kv_bytes = (layers as f64) * (heads_kv as f64) * (head_dim as f64) * (ctx as f64) * (bytes_per_elem_k + bytes_per_elem_v);
    let kv_cache_vram_gb = kv_bytes / (1024.0 * 1024.0 * 1024.0);

    // Embedding & Output head fixed tensors (~10% of total model weights)
    let base_tensors_gb = file_gb * 0.10;

    // Remaining tensor weights distributed across transformer layers (~90% of model size)
    let layer_tensors_total_gb = file_gb * 0.90;
    let vram_per_layer = layer_tensors_total_gb / (layers as f64);

    // Check total VRAM if 100% offloaded
    let model_full_vram_gb = file_gb * 1.04; // 4% CUDA memory allocation buffer
    let total_estimated_vram_gb = model_full_vram_gb + kv_cache_vram_gb;

    // Calculate maximum layers that fit inside usable_gpu_vram
    let vram_avail_for_layers = (usable_gpu_vram - base_tensors_gb - kv_cache_vram_gb).max(0.0);

    let can_fully_offload = total_estimated_vram_gb <= usable_gpu_vram;

    let recommended_ngl = if can_fully_offload {
        layers
    } else if vram_per_layer > 0.0 {
        ((vram_avail_for_layers / vram_per_layer).floor() as u32).min(layers)
    } else {
        0
    };

    let offload_percentage = ((recommended_ngl as f64 / layers as f64) * 100.0).min(100.0);

    VramEstimate {
        total_gpu_vram_gb,
        model_vram_gb: model_full_vram_gb,
        kv_cache_vram_gb,
        total_estimated_vram_gb,
        recommended_ngl,
        can_fully_offload,
        offload_percentage,
    }
}
