//! 通用图像解码：ImageReader → ICO 宽松 → 色调映射。
//!
//! 不含平台 Shell/GDI；应用层（cherry-box）负责最终兜底。

mod bmp;
pub(crate) mod ffmpeg_decode;
mod ffmpeg_image;
pub(crate) mod ffmpeg_log;
pub(crate) mod ffmpeg_probe;
pub mod subtitle;
pub mod timeline;
pub mod waveform;
mod ico;
mod j2k;
mod jxl;
mod mng;
mod reader;
pub mod tone_map;

use std::path::Path;
use std::sync::Once;

use image::DynamicImage;
use thiserror::Error;

static REGISTER_EXTRAS: Once = Once::new();

fn ensure_extras_registered() {
    REGISTER_EXTRAS.call_once(|| {
        image_extras::register();
    });
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("无法解码图像")]
    Unsupported,
}

/// 尽力解码任意支持的图像文件
pub fn decode_image(path: &Path) -> Result<DynamicImage, DecodeError> {
    ensure_extras_registered();
    #[cfg(feature = "video")]
    ffmpeg_log::init_ffmpeg_logging();
    reader::try_decode_reader(path)
        .or_else(|| jxl::try_decode_jxl(path))
        .or_else(|| j2k::try_decode_j2k(path))
        .or_else(|| j2k::try_decode_j2k_rgba(path))
        .or_else(|| j2k::try_decode_j2k_via_ffmpeg(path))
        .or_else(|| ico::try_decode_ico(path))
        .or_else(|| mng::try_decode_mng(path))
        .or_else(|| ffmpeg_image::try_decode_via_ffmpeg(path))
        .ok_or(DecodeError::Unsupported)
}

/// 解码并缩放到 max_dim 边长内
pub fn decode_and_thumbnail(path: &Path, max_dim: u32) -> Result<DynamicImage, DecodeError> {
    let img = decode_image(path)?;
    Ok(if img.width() > max_dim || img.height() > max_dim {
        img.thumbnail(max_dim, max_dim)
    } else {
        img
    })
}
