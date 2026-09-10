//! SVG 矢量图缩略：resvg 栅格化。

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use image::DynamicImage;
use resvg::tiny_skia::Pixmap;
use resvg::usvg::fontdb;
use resvg::usvg::{Options, Tree};

/// 进程级共享字体库：load_system_fonts() 需扫描系统字体目录、开销较大，
/// 缓存一份供所有 SVG 栅格化复用，避免批量生成缩略图时每个文件重复加载。
static FONT_DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// 构建带系统字体的共享 fontdb。
/// 若使用空的默认数据库，SVG 中 `<text>` 的字体匹配必然失败：
/// 文本不渲染（缩略图文字空白），且 usvg 每处文本刷一条 "No match for ... font-family" 警告。
fn shared_fontdb() -> Arc<fontdb::Database> {
    FONT_DB
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// 等比缩放至 max_dim 边长内栅格化 SVG
pub fn create_thumbnail(path: &Path, max_dim: u32) -> Result<DynamicImage> {
    let data = std::fs::read(path).with_context(|| format!("读取 SVG 失败: {:?}", path))?;
    let opt = Options {
        fontdb: shared_fontdb(),
        ..Options::default()
    };
    let tree = Tree::from_data(&data, &opt).context("解析 SVG 失败")?;
    let size_f = tree.size();
    // 大图缩小，小图不放大以免模糊
    let scale = (max_dim as f32 / size_f.width().max(size_f.height())).min(1.0);
    let w = (size_f.width() * scale).ceil() as u32;
    let h = (size_f.height() * scale).ceil() as u32;
    let mut pixmap = Pixmap::new(w, h).context("创建 pixmap 失败")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let img = DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(w, h, pixmap.data().to_vec()).context("构建 RGBA 失败")?,
    );
    Ok(img.thumbnail(max_dim, max_dim))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 带 sans-serif 文本的 SVG 应能命中系统字体并渲染出文字像素。
    /// 若字体库未生效，文本区域为全透明，此测试即失败。
    #[test]
    fn text_with_sans_serif_renders() {
        let path = std::env::temp_dir().join("media_decode_font_test.svg");
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
                <text x="5" y="30" font-family="sans-serif" font-size="24">Test</text>
            </svg>"##,
        )
        .unwrap();
        let img = create_thumbnail(&path, 64).unwrap();
        let has_ink = img.to_rgba8().pixels().any(|p| p.0[3] > 0);
        assert!(has_ink, "sans-serif 文本未渲染：系统字体库未生效");
    }
}
