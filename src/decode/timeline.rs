//! 视频时间轴缩略图：均匀 seek 抽帧并拼接 sprite，供 Quick Look Artplayer scrub 使用。

use std::path::Path;

use image::{DynamicImage, Rgba, RgbaImage};

use super::ffmpeg_decode;
use super::ffmpeg_probe;

/// 按时长均匀抽取最多 `count` 帧，每帧缩放到高度 `thumb_h`（保持宽高比）。
#[cfg(feature = "video")]
pub fn decode_video_timeline_frames(path: &Path, count: usize, thumb_h: u32) -> Vec<DynamicImage> {
    super::ffmpeg_log::init_ffmpeg_logging();
    let probe = ffmpeg_probe::probe_options_for_video(path);
    let duration = ffmpeg_decode::probe_duration_secs(path, probe.clone()).unwrap_or(0.0);
    let n = count.clamp(1, 40);
    let seek_points: Vec<f64> = if duration <= 0.5 {
        vec![0.0]
    } else {
        (0..n)
            .map(|i| {
                if n == 1 {
                    0.0
                } else {
                    duration * i as f64 / (n - 1) as f64
                }
            })
            .collect()
    };
    let mut frames = Vec::with_capacity(seek_points.len());
    for secs in seek_points {
        if let Ok(frame) = ffmpeg_decode::decode_video_first_frame(path, secs, probe.clone()) {
            let (w, h) = (frame.width(), frame.height());
            if h > 0 && w > 0 {
                let thumb_w = (w as f64 * thumb_h as f64 / h as f64).round() as u32;
                frames.push(frame.thumbnail(thumb_w.max(1), thumb_h));
            }
        }
    }
    if frames.is_empty() {
        if let Ok(frame) = ffmpeg_decode::decode_video_first_frame(path, 0.0, probe) {
            let (w, h) = (frame.width(), frame.height());
            if h > 0 {
                let thumb_w = (w as f64 * thumb_h as f64 / h as f64).round() as u32;
                frames.push(frame.thumbnail(thumb_w.max(1), thumb_h));
            }
        }
    }
    frames
}

/// 将帧拼成 Artplayer 所需 sprite 图（按行优先：row × column 网格）。
#[cfg(feature = "video")]
pub fn stitch_timeline_sprite(
    frames: &[DynamicImage],
    columns: u32,
    thumb_w: u32,
    thumb_h: u32,
) -> Option<RgbaImage> {
    if frames.is_empty() || columns == 0 {
        return None;
    }
    let cols = columns as usize;
    let rows = frames.len().div_ceil(cols);
    let mut canvas = RgbaImage::from_pixel(
        thumb_w * columns,
        thumb_h * rows as u32,
        Rgba([24, 24, 28, 255]),
    );
    for (i, frame) in frames.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let rgba = frame.to_rgba8();
        let (fw, fh) = (rgba.width(), rgba.height());
        let x0 = col as u32 * thumb_w + (thumb_w.saturating_sub(fw)) / 2;
        let y0 = row as u32 * thumb_h + (thumb_h.saturating_sub(fh)) / 2;
        for y in 0..fh.min(thumb_h) {
            for x in 0..fw.min(thumb_w) {
                let px = rgba.get_pixel(x, y);
                canvas.put_pixel(x0 + x, y0 + y, *px);
            }
        }
    }
    Some(canvas)
}

#[cfg(not(feature = "video"))]
pub fn decode_video_timeline_frames(_path: &Path, _count: usize, _thumb_h: u32) -> Vec<DynamicImage> {
    Vec::new()
}

#[cfg(not(feature = "video"))]
pub fn stitch_timeline_sprite(
    _frames: &[DynamicImage],
    _columns: u32,
    _thumb_w: u32,
    _thumb_h: u32,
) -> Option<RgbaImage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stitch_empty_returns_none() {
        assert!(stitch_timeline_sprite(&[], 10, 160, 90).is_none());
    }
}
