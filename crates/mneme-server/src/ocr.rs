//! 本地 OCR（RapidOCR / PP-OCRv4 ONNX 模型）。
//!
//! 设计约束：
//! - 模型与 onnxruntime.dll **不进 git**（20MB 二进制），由 `tools/fetch-ocr-models.py`
//!   从 PyPI 的 rapidocr-onnxruntime / onnxruntime wheel 解包到 `<数据目录>/models/`。
//!   找不到模型时 OCR 功能优雅降级（/api/ocr/info 报 ready=false），其余功能不受影响。
//! - ort 必须以 load-dynamic 方式运行：构建期不联网，运行时显式加载
//!   `<模型目录>/onnxruntime.dll`（EnvironmentBuilder::with_dynamic_library_from_path，
//!   必须在任何 Session 创建前 commit，所以惰性初始化里第一件事就是它）。
//! - 推理是 CPU 密集（几百 ms~秒级），走 spawn_blocking，不卡 axum 运行时；
//!   桌面壳的全局热键（截图识字）也调同一份 [`ocr_bytes_blocking`]。
//! - 长图（>2000px 高）自动按 2000px+120px 重叠切片逐块识别再按序拼接：
//!   det 的 limit_side_len=min 模式会把大图按短边归一，整页长图直接缩没字。

use base64::Engine;
use paddle_ocr_rs::ocr_lite::OcrLite;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use std::path::{Path, PathBuf};
use std::sync::{Mutex as StdMutex, Once, OnceLock};

/// 模型文件名（RapidOCR 官方命名，fetch 脚本按这些名字解包）。
const DET_ONNX: &str = "ch_PP-OCRv4_det_infer.onnx";
const CLS_ONNX: &str = "ch_ppocr_mobile_v2.0_cls_infer.onnx";
const REC_ONNX: &str = "ch_PP-OCRv4_rec_infer.onnx";
const ORT_DLL: &str = if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" };
static OCR: StdMutex<Option<OcrLite>> = StdMutex::new(None);
/// 命中的模型目录，info 接口用来提示。
static OCR_DIR: OnceLock<PathBuf> = OnceLock::new();
/// 初始化失败原因（模型缺失 / dll 加载失败），info 接口透出。
static OCR_ERR: StdMutex<Option<String>> = StdMutex::new(None);

/// 模型目录搜索顺序：环境变量 → 数据目录/models → exe 同级/models → 工作目录/models。
fn model_dirs(data_root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(e) = std::env::var("MNEME_OCR_MODELS") {
        if !e.trim().is_empty() {
            v.push(PathBuf::from(e));
        }
    }
    v.push(data_root.join("models"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push(d.join("models"));
        }
    }
    v.push(PathBuf::from("models"));
    v
}

fn required_files() -> [(&'static str, &'static str); 4] {
    [(DET_ONNX, "检测"), (CLS_ONNX, "方向"), (REC_ONNX, "识别"), (ORT_DLL, "运行时")]
}

/// 查找齐备的模型目录；返回 (目录, 缺失列表)。
/// 找到全齐的目录 → (Some(dir), [])；有目录但不齐 → (None, 缺失文件)；
/// 一个模型都没有 → (None, 全部四个文件名)。
pub fn locate(data_root: &Path) -> (Option<PathBuf>, Vec<&'static str>) {
    let all_files: Vec<&'static str> = required_files().iter().map(|(f, _)| *f).collect();
    let mut partial_missing: Option<Vec<&'static str>> = None;
    for dir in model_dirs(data_root) {
        let missing: Vec<&'static str> =
            all_files.iter().copied().filter(|f| !dir.join(f).is_file()).collect();
        if missing.is_empty() {
            return (Some(dir), Vec::new());
        }
        if missing.len() < all_files.len() && partial_missing.is_none() {
            partial_missing = Some(missing);
        }
    }
    (None, partial_missing.unwrap_or_else(|| all_files.clone()))
}

/// 惰性初始化：定位模型 → ort 环境加载 dll → 三个模型建 Session。
/// 已成功初始化则直接返回 Ok。
fn ensure_engine(data_root: &Path) -> Result<(), String> {
    let mut guard = OCR.lock().map_err(|_| "OCR 锁中毒")?;
    if guard.is_some() {
        return Ok(());
    }
    let (dir, missing) = locate(data_root);
    let Some(dir) = dir else {
        let msg = if missing.is_empty() {
            "未找到 OCR 模型目录".to_string()
        } else {
            format!("OCR 模型缺失：{}（运行 tools/fetch-ocr-models.py 下载）", missing.join("、"))
        };
        *OCR_ERR.lock().unwrap() = Some(msg.clone());
        return Err(msg);
    };
    let _ = OCR_DIR.set(dir.clone());
    let dll = dir.join(ORT_DLL);
    // ort 全局环境只允许 commit 一次；这里失败说明 dll 路径不对或版本不兼容。
    static ORT_ENV: Once = Once::new();
    let mut env_err: Option<String> = None;
    ORT_ENV.call_once(|| {
        if let Err(e) = ort::init_from(dll.to_string_lossy().as_ref()).commit() {
            env_err = Some(format!("onnxruntime.dll 加载失败（{}）: {e}", dll.display()));
        }
    });
    if let Some(msg) = env_err {
        *OCR_ERR.lock().unwrap() = Some(msg.clone());
        return Err(msg);
    }
    let mut ocr = OcrLite::new();
    let load = |ocr: &mut OcrLite| -> Result<(), String> {
        ocr.init_models(
            dir.join(DET_ONNX).to_str().ok_or("模型路径含非 UTF-8")?,
            dir.join(CLS_ONNX).to_str().ok_or("模型路径含非 UTF-8")?,
            dir.join(REC_ONNX).to_str().ok_or("模型路径含非 UTF-8")?,
            // 0 = 用 onnxruntime 默认线程数；本机单用户场景交给运行时自己调
            0,
        )
        .map_err(|e| format!("模型加载失败: {e}"))
    };
    if let Err(msg) = load(&mut ocr) {
        *OCR_ERR.lock().unwrap() = Some(msg.clone());
        return Err(msg);
    }
    *guard = Some(ocr);
    *OCR_ERR.lock().unwrap() = None;
    Ok(())
}

/// GET /api/ocr/info —— 桌面端据此决定亮不亮 OCR 入口。
pub fn ocr_info(data_root: &Path) -> JsonValue {
    // info 也顺带触发惰性初始化（模型齐备时 <1s），否则面板会在首次推理前
    // 一直显示「未就绪」，误导用户。
    let init = ensure_engine(data_root);
    let (dir, missing) = locate(data_root);
    let ready = init.is_ok();
    json!({
        "ready": ready,
        "dir": dir.map(|d| d.display().to_string()),
        "missing": missing,
        "error": init.err().or_else(|| OCR_ERR.lock().ok().and_then(|e| e.clone())),
        "hint": "运行 tools/fetch-ocr-models.py 下载模型（约 16MB，放 数据目录/models/）",
    })
}

#[derive(Deserialize)]
pub struct OcrBody {
    /// base64 编码的图片字节（PNG/JPEG/BMP/WebP），前端 FileReader 读 dataURL 后去前缀。
    pub b64: String,
    /// 是否跑方向分类（截图基本不倒转，默认关省一次推理）。
    #[serde(default)]
    pub angle: bool,
}

/// 一次识别的结果：整段文本 + 逐行（文本, 置信度）+ 耗时。
pub struct OcrOutcome {
    pub text: String,
    pub lines: Vec<(String, f32)>,
    pub ms: u64,
}

/// 阻塞式识别入口：HTTP 路由与桌面壳的全局热键共用这一份。
/// 调用方负责放到不阻塞 UI/异步运行时的线程里。
pub fn ocr_bytes_blocking(data_root: &Path, bytes: &[u8], do_angle: bool) -> Result<OcrOutcome, String> {
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("图片超过 20MB".into());
    }
    ensure_engine(data_root)?;
    let img = image::load_from_memory(bytes)
        .map_err(|e| format!("图片解码失败: {e}"))?
        .to_rgb8();
    // 小图放大：PP-OCR 检测对 <30px 高的行基本失明；放大到合理尺寸再进模型。
    let (w, h) = (img.width(), img.height());
    let img = if h < 32 && h > 0 {
        let scale = (96.0 / h as f32).min(4.0);
        image::imageops::resize(
            &img,
            ((w as f32) * scale).max(1.0) as u32,
            ((h as f32) * scale).max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let t0 = std::time::Instant::now();
    // 长图分块：det 的 limit_side_len=736(min) 会把大图按短边归一，
    // 900x5000 的整页笔记缩完文字只有几个像素高，整页必掉行。
    // 按高 2000px + 120px 重叠切片逐块识别再按序拼接，块内顺序由检测器保证。
    const SLICE_H: u32 = 2000;
    const OVERLAP: u32 = 120;
    let mut outcome = OcrOutcome { text: String::new(), lines: Vec::new(), ms: 0 };
    let slices: Vec<image::RgbImage> = if img.height() > SLICE_H + OVERLAP {
        let mut v = Vec::new();
        let mut top = 0u32;
        while top < img.height() {
            let bottom = (top + SLICE_H).min(img.height());
            v.push(image::imageops::crop_imm(&img, 0, top, img.width(), bottom - top).to_image());
            if bottom >= img.height() {
                break;
            }
            top = bottom - OVERLAP;
        }
        v
    } else {
        vec![img]
    };
    for (i, slice) in slices.iter().enumerate() {
        let part = detect_one(slice, do_angle)?;
        if i > 0 {
            outcome.text.push('\n');
        }
        outcome.text.push_str(&part.text);
        outcome.lines.extend(part.lines);
    }
    outcome.ms = t0.elapsed().as_millis() as u64;
    Ok(outcome)
}

/// 单块推理。参数与 RapidOCR 默认一致：padding 50 / 最长边 2000 / 置信度 0.5 / 阈值 0.3 / 膨胀 1.6。
fn detect_one(img: &image::RgbImage, do_angle: bool) -> Result<OcrOutcome, String> {
    let mut guard = OCR.lock().map_err(|_| "OCR 锁中毒".to_string())?;
    let Some(ocr) = guard.as_mut() else {
        return Err("OCR 未初始化".into());
    };
    let res = ocr
        .detect(img, 50, 2000, 0.5, 0.3, 1.6, do_angle, false)
        .map_err(|e| format!("识别失败: {e}"))?;
    // 文本块已是检测顺序（上→下、行内左→右）。
    Ok(OcrOutcome {
        text: res.text_blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n"),
        lines: res.text_blocks.iter().map(|b| (b.text.clone(), b.text_score)).collect(),
        ms: 0,
    })
}

/// POST /api/ocr —— 识别图片文字，返回按阅读顺序拼好的文本与逐行明细。
pub async fn ocr_run(
    data_root: PathBuf,
    body: OcrBody,
) -> JsonValue {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(body.b64.trim()) {
        Ok(b) => b,
        Err(e) => return json!({ "error": format!("base64 解码失败: {e}") }),
    };
    let angle = body.angle;
    let root = data_root.clone();
    let result = tokio::task::spawn_blocking(move || ocr_bytes_blocking(&root, &bytes, angle)).await;
    match result {
        Ok(Ok(o)) => json!({
            "text": o.text,
            "lines": o.lines.iter().map(|(t, s)| json!({ "text": t, "score": s })).collect::<Vec<_>>(),
            "ms": o.ms,
        }),
        Ok(Err(msg)) => json!({ "error": msg }),
        Err(e) => json!({ "error": format!("OCR 线程异常: {e}") }),
    }
}
