mod decode;
pub mod frame_valid;
mod meta;
mod mime_resolve;
mod thumbs;
pub mod types;

pub use decode::{DecodeError, decode_and_thumbnail, decode_image};
pub use frame_valid::{is_effectively_blank, is_rgba_blank};
pub use meta::{MediaMeta, probe_media_meta};

use std::{fs::File, path::Path, str::FromStr};

use ::image::{DynamicImage, ImageFormat, codecs::jpeg::JpegEncoder};
use strum_macros::{AsRefStr, Display, EnumString};

#[derive(thiserror::Error, Debug)]
pub enum ThumbnailError {
    #[error("IOError")]
    IOError(#[from] std::io::Error),
    #[error("ImageError")]
    ImageError(#[from] ::image::ImageError),
    #[error("PngError")]
    PngError(#[from] oxipng::PngError),
    #[error("AnyError")]
    AnyError(#[from] anyhow::Error),
    #[error("Unsupported MIME type:`{0}`")]
    UnsupportedError(String),
}

#[derive(Debug, Copy, Clone, Display, EnumString, AsRefStr)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum Encoding {
    Jpeg,
    Png,
    Webp,
}

/// PSD 预览提取策略；调用方可在准确性和扫描开销之间显式取舍。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PsdPreviewPolicy {
    /// 优先解析完整 composite，失败或超限时再回退到 IRB。
    #[default]
    PreferComposite,
    /// 只读取约 160px 的内嵌 IRB，避免为扫描主色完整加载大文件。
    IrbOnly,
}

/// Represents fixed sizes of a thumbnail
#[derive(Clone, Copy, Debug)]
pub enum ThumbnailSize {
    Icon,
    Small,
    Medium,
    Large,
    Larger,
    Custom((u32, u32)),
}

impl ThumbnailSize {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            ThumbnailSize::Icon => (64, 64),
            ThumbnailSize::Small => (128, 128),
            ThumbnailSize::Medium => (256, 256),
            ThumbnailSize::Large => (512, 512),
            ThumbnailSize::Larger => (1024, 1024),
            ThumbnailSize::Custom(size) => *size,
        }
    }
}

/// 按 MIME/扩展名路由解码并缩放到 max_dim 边长内（svg/raw/audio/office 优先于通用 image）
pub fn decode_for_thumbnail(path: &Path, max_dim: u32) -> Result<DynamicImage, DecodeError> {
    decode_for_thumbnail_with_psd_policy(path, max_dim, PsdPreviewPolicy::PreferComposite)
}

/// 按 MIME/扩展名路由解码，并允许 PSD 调用方选择是否解析完整 composite。
///
/// 非 PSD 格式会忽略 `psd_policy`，从而保持统一入口而不改变既有路由行为。
pub fn decode_for_thumbnail_with_psd_policy(
    path: &Path,
    max_dim: u32,
    psd_policy: PsdPreviewPolicy,
) -> Result<DynamicImage, DecodeError> {
    let mime = mime_resolve::resolve_mime(path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    #[cfg(feature = "svg")]
    if mime_resolve::is_svg_mime(&mime) || ext == "svg" {
        use crate::thumbs::svg;
        return svg::create_thumbnail(path, max_dim).map_err(|_| DecodeError::Unsupported);
    }

    #[cfg(feature = "raw")]
    if mime_resolve::is_raw_ext(&ext) {
        use crate::thumbs::raw;
        return raw::create_thumbnail(path, max_dim).map_err(|_| DecodeError::Unsupported);
    }

    #[cfg(feature = "audio")]
    if mime_resolve::is_audio_mime(&mime) || mime_resolve::is_audio_ext(&ext) {
        use crate::thumbs::audio;
        return audio::extract_cover(path, max_dim).ok_or(DecodeError::Unsupported);
    }

    #[cfg(feature = "office")]
    if mime_resolve::is_office_mime(&mime) || mime_resolve::is_office_ext(&ext) {
        use crate::thumbs::office;
        return office::create_thumbnail(path, max_dim).ok_or(DecodeError::Unsupported);
    }

    #[cfg(feature = "source")]
    if crate::thumbs::source::is_source_ext(&ext) {
        use crate::thumbs::source;
        return source::create_thumbnail_with_psd_policy(path, max_dim, psd_policy)
            .ok_or(DecodeError::Unsupported);
    }

    decode_and_thumbnail(path, max_dim)
}

/// 读取 Office ZIP 内 EMF 缩略图原始字节（供应用层 GDI 栅格化）
#[cfg(feature = "office")]
pub fn extract_office_emf_bytes(path: &Path) -> Option<Vec<u8>> {
    crate::thumbs::office::extract_emf_bytes(path)
}

/// 逐页提取 PDF 文本（语义搜索/内容索引用）：返回按页序的文本，无文本层页为空串。
/// 内部复用与缩略图渲染同一把 pdfium 全局锁，宿主无需关心 FFI 线程安全；
/// 绑定失败或文档损坏返回 None。
#[cfg(feature = "pdf")]
pub fn extract_pdf_pages_text(path: &Path, max_pages: usize) -> Option<Vec<String>> {
    crate::thumbs::pdf::extract_pages_text(path, max_pages)
}

pub struct Thumbnailer {
    /// The maximum output width.
    pub width: u32,
    /// The maximum output height.
    pub height: u32,
    /// Encode the image with the given quality.
    /// Only support Jpeg and Webp.
    /// The image quality must be between 1 and 100 inclusive for minimal and maximal quality respectively.
    pub quality: u8,
}

impl Default for Thumbnailer {
    fn default() -> Self {
        Self::new(ThumbnailSize::Medium, 90)
    }
}

impl Thumbnailer {
    pub fn new(size: ThumbnailSize, quality: u8) -> Self {
        let (width, height) = size.dimensions();
        Self {
            width,
            height,
            quality,
        }
    }

    /// create thumbnail image.
    /// path: source file path.
    /// output: thumbnail image path.
    pub fn create_thumbnail<P, T>(
        &'_ self,
        path: P,
        output: T,
    ) -> anyhow::Result<(), ThumbnailError>
    where
        P: AsRef<Path>,
        T: AsRef<Path>,
    {
        let path = path.as_ref();
        let mime = mime_resolve::resolve_mime(path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let encoding = output
            .as_ref()
            .extension()
            .and_then(|ext| ext.to_ascii_uppercase().to_str().map(str::to_string))
            .and_then(|ext| Encoding::from_str(&ext).ok())
            .unwrap_or_else(|| {
                log::debug!("Defaulting encoding to Jpeg");
                Encoding::Jpeg
            });

        let max_dim = self.width.max(self.height);

        #[cfg(feature = "svg")]
        if mime_resolve::is_svg_mime(&mime) || ext == "svg" {
            use crate::thumbs::svg;
            let img = svg::create_thumbnail(path, max_dim)?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "raw")]
        if mime_resolve::is_raw_ext(&ext) {
            use crate::thumbs::raw;
            let img = raw::create_thumbnail(path, max_dim)?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "audio")]
        if mime_resolve::is_audio_mime(&mime) || mime_resolve::is_audio_ext(&ext) {
            use crate::thumbs::audio;
            let img = audio::extract_cover(path, max_dim)
                .ok_or_else(|| ThumbnailError::UnsupportedError(mime.clone()))?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "office")]
        if mime_resolve::is_office_mime(&mime) || mime_resolve::is_office_ext(&ext) {
            use crate::thumbs::office;
            let img = office::create_thumbnail(path, max_dim)
                .ok_or_else(|| ThumbnailError::UnsupportedError(mime.clone()))?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "source")]
        if crate::thumbs::source::is_source_ext(&ext) {
            use crate::thumbs::source;
            let img = source::create_thumbnail(path, max_dim)
                .ok_or_else(|| ThumbnailError::UnsupportedError(mime.clone()))?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "image")]
        if mime_resolve::is_image_mime(&mime) {
            use crate::thumbs::image;

            let img = image::create_thumbnail(path, self.width, self.height)?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "pdf")]
        if mime_resolve::is_pdf_mime(&mime) {
            use crate::thumbs::pdf;

            let img = pdf::create_thumbnail(path, self.width, self.height)?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        #[cfg(feature = "video")]
        if mime_resolve::is_video_mime(&mime) {
            use crate::thumbs::video;

            let img = video::create_thumbnail(path, self.width, self.height)?;
            self.encod_and_save(img, encoding, output)?;
            return Ok(());
        }

        Err(ThumbnailError::UnsupportedError(mime))
    }

    fn encod_and_save<P>(
        &'_ self,
        img: DynamicImage,
        encoding: Encoding,
        output: P,
    ) -> anyhow::Result<(), ThumbnailError>
    where
        P: AsRef<Path>,
    {
        match encoding {
            Encoding::Jpeg => {
                let output = File::create(output)?;
                let encoder = JpegEncoder::new_with_quality(output, self.quality);
                img.write_with_encoder(encoder)?;
            }
            Encoding::Png => {
                img.save_with_format(&output, ImageFormat::Png)?;

                oxipng::optimize(
                    &oxipng::InFile::Path(output.as_ref().to_path_buf()),
                    &oxipng::OutFile::from_path(output.as_ref().to_path_buf()),
                    &oxipng::Options::max_compression(),
                )?;
            }
            Encoding::Webp => {
                let rgba = img.to_rgba8();
                let encoder = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
                let memory = encoder.encode(self.quality.into());
                std::fs::write(output, &*memory)?;
            }
        };

        Ok(())
    }
}
