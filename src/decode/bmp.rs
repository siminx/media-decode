//! BMP 偏移容错解码：修复文件头 bfOffBits 坏值后重试。
//!
//! image crate 的 BMP 解码器无条件信任文件头的像素数据偏移（bfOffBits），
//! 部分工具写出的 BMP 该字段是垃圾值（如指向图像中部），seek 后读像素数据
//! 必然 EOF；而 Windows 自带解码器（GDI+/WIC）对这类文件有容错。本模块仅在
//! 标准解码失败后介入：检测偏移异常时按候选偏移改写头部再解码，以试解码
//! 结果为准，避免误修正常文件。

use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
};

use image::{DynamicImage, ImageFormat, ImageReader};

use super::tone_map::apply_tone_map_if_needed;

/// BMP 文件头 14 字节 + 解析所需的最长 DIB 字段（clrUsed 位于 40 字节头内）
const HEAD_LEN: usize = 54;
/// 修复需要整文件读入内存，超过该大小视为异常文件直接放弃
const MAX_REPAIR_SIZE: u64 = 512 * 1024 * 1024;

/// 尝试修复 bfOffBits 坏值的 BMP 并解码；正常文件在标准解码已成功，不会进入这里
pub(crate) fn try_decode_bmp_repaired(path: &Path) -> Option<DynamicImage> {
    let mut file = File::open(path).ok()?;
    let file_size = file.metadata().ok()?.len();
    if file_size < HEAD_LEN as u64 || file_size > MAX_REPAIR_SIZE {
        return None;
    }
    let mut head = [0u8; HEAD_LEN];
    file.read_exact(&mut head).ok()?;
    if &head[..2] != b"BM" {
        return None;
    }

    let bad_offset = u32::from_le_bytes(head[10..14].try_into().ok()?) as usize;
    let dib_size = u32::from_le_bytes(head[14..18].try_into().ok()?);
    // DIB 头大小只可能是 12/40/52/56/64/108/124，越界说明不是可信 BMP
    if !(12..=124).contains(&dib_size) {
        return None;
    }
    // BITMAPCOREHEADER 字段更窄且无 compression（恒为 BI_RGB）
    let (width, height, bpp, compression) = if dib_size == 12 {
        (
            u16::from_le_bytes(head[18..20].try_into().ok()?) as i32,
            u16::from_le_bytes(head[20..22].try_into().ok()?) as i32,
            u16::from_le_bytes(head[24..26].try_into().ok()?),
            0u32,
        )
    } else {
        (
            i32::from_le_bytes(head[18..22].try_into().ok()?),
            i32::from_le_bytes(head[22..26].try_into().ok()?),
            u16::from_le_bytes(head[28..30].try_into().ok()?),
            u32::from_le_bytes(head[30..34].try_into().ok()?),
        )
    };
    // image crate 最大支持 65535，超界或非法尺寸交回上层处理
    if width <= 0 || height == 0 || width > 0xFFFF || height.unsigned_abs() > 0xFFFF {
        return None;
    }

    // 仅无压缩（BI_RGB/BI_BITFIELDS）能精确算出像素流期望大小，
    // 以此判断 bfOffBits 是否异常；压缩格式无法验证，不做修复
    let pixel_size = match compression {
        0 | 3 => {
            // 行按 4 字节边界对齐（ceil(w * bpp / 32) * 4 字节）
            let row = (width as usize * bpp as usize).div_ceil(32) * 4;
            row.checked_mul(height.unsigned_abs() as usize)?
        }
        _ => return None,
    };
    // 偏移本身能容纳完整像素流：失败另有原因（变体不支持等），修复无意义
    if bad_offset
        .checked_add(pixel_size)
        .is_some_and(|end| end <= file_size as usize)
    {
        return None;
    }

    // 候选偏移：header_end + 调色板/BITFIELDS 掩码（多数文件的正确位置）、
    // 经典值 54 与裸 header_end；试解码本身就是最终验证，估算不必精确
    let header_end = 14 + dib_size as usize;
    let mut candidates = vec![
        header_end + palette_size(dib_size, bpp, compression, &head),
        54,
        header_end,
    ];
    candidates.sort_unstable();
    candidates.dedup();

    let mut data = std::fs::read(path).ok()?;
    for off in candidates {
        if off < header_end {
            continue;
        }
        match off.checked_add(pixel_size) {
            Some(end) if end <= data.len() => {}
            _ => continue,
        }
        data[10..14].copy_from_slice(&(off as u32).to_le_bytes());
        if let Ok(img) = ImageReader::with_format(Cursor::new(&data), ImageFormat::Bmp).decode() {
            return Some(apply_tone_map_if_needed(img));
        }
    }
    None
}

/// DIB 头与像素数据之间的调色板 / BITFIELDS 掩码字节数
fn palette_size(dib_size: u32, bpp: u16, compression: u32, head: &[u8]) -> usize {
    let mask_size: usize = if (compression == 3 || compression == 6) && (bpp == 16 || bpp == 32) {
        12 // BI_BITFIELDS 的 R/G/B 掩码紧跟 DIB 头
    } else {
        0
    };
    let (entries, entry_size) = if dib_size == 12 {
        // BITMAPCOREHEADER 无 clrUsed，固定 2^bpp 项，每项 3 字节
        (if bpp <= 8 { 1usize << bpp } else { 0 }, 3)
    } else if bpp <= 8 {
        let clr_used = head
            .get(46..50)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
            .unwrap_or(0);
        (if clr_used == 0 { 1usize << bpp } else { clr_used }, 4)
    } else {
        (0, 4)
    };
    mask_size.saturating_add(entries.saturating_mul(entry_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    #[test]
    fn repairs_corrupted_pixel_offset() {
        // 生成已知内容的 24bpp BMP
        let mut img = RgbImage::new(33, 21);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, ((x + y) % 256) as u8]);
        }
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img.clone())
            .write_to(&mut encoded, ImageFormat::Bmp)
            .unwrap();
        let mut data = encoded.into_inner();

        // 篡改 bfOffBits 为指向图像中部的垃圾值（模拟真实坏文件 3.bmp）
        data[10..14].copy_from_slice(&1024u32.to_le_bytes());

        let dir = std::env::temp_dir().join("media-decode-bmp-repair-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("corrupted_offset.bmp");
        std::fs::write(&path, &data).unwrap();

        // 标准解码必须失败，确保测试覆盖的是修复路径
        assert!(
            ImageReader::open(&path)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .decode()
                .is_err()
        );

        let repaired = try_decode_bmp_repaired(&path).expect("偏移坏值应可修复");
        assert_eq!((repaired.width(), repaired.height()), (33, 21));
        assert_eq!(repaired.to_rgb8().as_raw(), img.as_raw());
        let _ = std::fs::remove_file(&path);
    }
}
