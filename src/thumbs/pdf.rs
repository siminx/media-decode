//! PDF 首页渲染与页面尺寸探测。pdfium-render 的 BINDINGS 只能 set 一次，且 FFI 非线程安全，
//! 必须单次绑定 + 全局互斥，避免并发 bind 断言 panic / 堆损坏。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

use image::GenericImageView;
use image::DynamicImage;
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

/// pdfium 绑定与渲染共用一把锁：绑定只能成功一次，渲染也不能并发
static PDFIUM_LOCK: Mutex<PdfiumBindState> = Mutex::new(PdfiumBindState::Unbound);

enum PdfiumBindState {
    Unbound,
    Ready,
    Failed,
}

pub(crate) fn create_thumbnail<P>(path: P, width: u32, height: u32) -> anyhow::Result<DynamicImage>
where
    P: AsRef<Path>,
{
    let path = path.as_ref().to_path_buf();
    with_pdfium(|pdfium| render_first_page(pdfium, &path, width, height))
        .ok_or_else(|| anyhow::anyhow!("pdfium 渲染失败（库绑定失败或渲染出错/panic）"))
}

/// 探测 PDF 首页页面尺寸（pt 取整）；供 AI 等可被 pdfium 解析的封装格式读取画板尺寸
pub(crate) fn probe_page_size(path: &Path) -> Option<(u32, u32)> {
    with_pdfium(|pdfium| {
        let document = pdfium.load_pdf_from_file(path, None)?;
        let first_page = document.pages().first()?;
        // PdfPoints 的 value 字段为 f32（单位 pt），取整后作为像素展示值
        let width = first_page.width().value.round() as u32;
        let height = first_page.height().value.round() as u32;
        Ok((width, height))
    })
}

/// 确保绑定（进程内一次）后在锁内执行任务；pdfium FFI 非线程安全，所有调用必须串行。
/// 任务返回 Err 或发生 panic 统一归为 None，调用方按"不可解析"处理即可
fn with_pdfium<T>(task: impl FnOnce(&Pdfium) -> anyhow::Result<T>) -> Option<T> {
    let mut state = PDFIUM_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if matches!(*state, PdfiumBindState::Unbound) {
        *state = if bind_once() {
            PdfiumBindState::Ready
        } else {
            PdfiumBindState::Failed
        };
    }
    if matches!(*state, PdfiumBindState::Failed) {
        return None;
    }

    // 任务也在锁内：pdfium FFI 非线程安全
    catch_unwind(AssertUnwindSafe(|| {
        let pdfium = Pdfium::default();
        task(&pdfium).ok()
    }))
    .ok()
    .flatten()
}

/// 进程内只尝试绑定一次；已初始化时 BINDINGS.set 会断言，需 catch 后复用 default
fn bind_once() -> bool {
    let bind_result = catch_unwind(|| {
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
            .or_else(|_| Pdfium::bind_to_system_library())
    });
    match bind_result {
        Ok(Ok(_)) => true,
        Ok(Err(_)) => false,
        // 断言失败 = 全局 BINDINGS 已 set，可走 Pdfium::default()
        Err(_) => catch_unwind(|| {
            let _ = Pdfium::default();
        })
        .is_ok(),
    }
}

fn render_first_page(
    pdfium: &Pdfium,
    path: &Path,
    width: u32,
    height: u32,
) -> anyhow::Result<DynamicImage> {
    let document = pdfium.load_pdf_from_file(path, None)?;
    let render_config = PdfRenderConfig::new();
    let first_page = document.pages().first()?;
    let img = first_page.render_with_config(&render_config)?.as_image()?;
    Ok(img.thumbnail(width, height))
}

/// 逐页渲染 PDF 为位图（供 OCR 评估/扫描件兜底）；与文本提取共用 pdfium 全局锁。
pub fn render_pdf_pages(path: &Path, max_pages: usize) -> Option<Vec<DynamicImage>> {
    render_pdf_pages_with_limit(path, max_pages, None)
}

/// OCR 专用逐页渲染：默认 72 DPI 位图再按 `scale` 放大（2.0 ≈ PyMuPDF Matrix(2,2)）。
pub fn render_pdf_pages_for_ocr(
    path: &Path,
    max_pages: usize,
    scale: f32,
) -> Option<Vec<DynamicImage>> {
    let scale = scale.max(1.0);
    render_pdf_pages_with_limit(path, max_pages, None).map(|pages| {
        pages
            .into_iter()
            .map(|img| {
                let (w, h) = img.dimensions();
                let nw = ((w as f32 * scale).round() as u32).max(1);
                let nh = ((h as f32 * scale).round() as u32).max(1);
                if nw == w && nh == h {
                    return img;
                }
                image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle).into()
            })
            .collect()
    })
}

/// 逐页渲染 PDF；`max_long_side` 为 Some 时将长边压至不超过该像素（OCR 评估常用 1680）。
pub fn render_pdf_pages_with_limit(
    path: &Path,
    max_pages: usize,
    max_long_side: Option<u32>,
) -> Option<Vec<DynamicImage>> {
    with_pdfium(|pdfium| {
        let document = pdfium.load_pdf_from_file(path, None)?;
        let render_config = PdfRenderConfig::new();
        let mut pages = Vec::new();
        for page in document.pages().iter().take(max_pages) {
            let mut img = page.render_with_config(&render_config)?.as_image()?;
            if let Some(limit) = max_long_side {
                let (w, h) = img.dimensions();
                if w.max(h) > limit {
                    img = img.thumbnail(limit, limit);
                }
            }
            pages.push(img);
        }
        Ok(pages)
    })
}

/// 逐页提取 PDF 文本（供宿主应用做文档 embedding / 内容 FTS）。
/// 返回按页序排列的文本（无文本层的页为空串），便于调用方按页做"扫描件判定"
/// （整本几乎无字符 → 无文本层）与按页分块。
/// 与渲染共用同一把 pdfium 全局锁 + 单次绑定：pdfium FFI 非线程安全。
pub(crate) fn extract_pages_text(path: &Path, max_pages: usize) -> Option<Vec<String>> {
    with_pdfium(|pdfium| {
        let document = pdfium.load_pdf_from_file(path, None)?;
        let mut pages = Vec::new();
        for page in document.pages().iter().take(max_pages) {
            // 单页文本失败不终止整本：损坏页按无文本处理
            // pdfium-render 0.9 的 text.all() 直接返回 String（空则空串），
            // 页面损坏/无文本时按空页处理（扫描件判定的构成部分）
            let text = match page.text() {
                Ok(text) => text.all(),
                Err(_) => String::new(),
            };
            pages.push(text);
        }
        Ok(pages)
    })
}
