//! 音频波形缩略图：解码采样峰值并渲染对称波形条，供列表 Icon 档展示。

use std::path::Path;

use image::{Rgba, RgbaImage};

use super::ffmpeg_log;

/// 默认波形条数量（Icon 档 512×307 5:3：256 条 × 2px 步进 ≈ 1px 柱 + 1px 间距）
pub const DEFAULT_WAVEFORM_BARS: usize = 256;

const BAR_WIDTH: u32 = 1;
const BAR_GAP: u32 = 1;
const BG_COLOR: Rgba<u8> = Rgba([0, 0, 0, 0]);
const BAR_COLOR: Rgba<u8> = Rgba([124, 108, 255, 255]);

/// 波形绘制样式（Icon 档默认透明底紫条）
#[derive(Clone, Copy, Debug)]
pub struct WaveformStyle {
    pub background: Rgba<u8>,
    pub bar: Rgba<u8>,
    pub bar_width: u32,
    pub bar_gap: u32,
}

impl Default for WaveformStyle {
    fn default() -> Self {
        Self {
            background: BG_COLOR,
            bar: BAR_COLOR,
            bar_width: BAR_WIDTH,
            bar_gap: BAR_GAP,
        }
    }
}

/// 从音频文件解码采样并渲染波形 RGBA 图；失败返回 None（上层回退专辑封面/扩展名图标）。
#[cfg(feature = "video")]
pub fn render_audio_waveform(
    path: &Path,
    width: u32,
    height: u32,
    bars: usize,
) -> Option<RgbaImage> {
    ffmpeg_log::init_ffmpeg_logging();
    let target_bars = bars.max(16).min(512);
    let peaks = decode_audio_peaks(path, target_bars)?;
    if peaks.is_empty() {
        return None;
    }
    Some(draw_waveform(&peaks, width.max(32), height.max(32), WaveformStyle::default()))
}

/// 排空解码器帧缓冲；返回 true 表示已达采样上限
#[cfg(feature = "video")]
fn drain_decoder_frames(
    path: &Path,
    decoder: &mut ffmpeg_next::decoder::Audio,
    resampler: &mut Option<ffmpeg_next::software::resampling::context::Context>,
    decoded: &mut ffmpeg_next::util::frame::Audio,
    resampled: &mut ffmpeg_next::util::frame::Audio,
    samples: &mut Vec<f32>,
    max_samples: usize,
) -> bool {
    while decoder.receive_frame(decoded).is_ok() {
        if let Some(res) = resampler.as_mut() {
            match res.run(decoded, resampled) {
                Ok(_) => append_frame_mono_f32(samples, resampled, max_samples),
                Err(e) => {
                    log::debug!("波形 resample 失败 {:?}: {e}", path);
                    append_frame_mono_f32(samples, decoded, max_samples);
                }
            }
        } else {
            append_frame_mono_f32(samples, decoded, max_samples);
        }
        if samples.len() >= max_samples {
            return true;
        }
    }
    false
}

/// 将任意常见 PCM 帧转为单声道 f32 追加到 samples
#[cfg(feature = "video")]
fn append_frame_mono_f32(
    samples: &mut Vec<f32>,
    frame: &ffmpeg_next::util::frame::Audio,
    max_samples: usize,
) {
    use ffmpeg_next::format::sample::{Sample, Type};

    let nb = frame.samples();
    if nb == 0 {
        return;
    }
    let channels = frame.channels().max(1) as usize;

    match frame.format() {
        Sample::F32(Type::Packed) => {
            let data = frame.data(0);
            for i in 0..nb {
                if samples.len() >= max_samples {
                    break;
                }
                let offset = i * 4;
                if offset + 4 > data.len() {
                    break;
                }
                samples.push(f32::from_le_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]));
            }
        }
        Sample::F32(Type::Planar) => {
            for i in 0..nb {
                if samples.len() >= max_samples {
                    break;
                }
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let data = frame.data(ch);
                    let offset = i * 4;
                    if offset + 4 <= data.len() {
                        sum += f32::from_le_bytes([
                            data[offset],
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                        ]);
                    }
                }
                samples.push(sum / channels as f32);
            }
        }
        Sample::I16(Type::Packed) => {
            let data = frame.data(0);
            for i in 0..nb {
                if samples.len() >= max_samples {
                    break;
                }
                let offset = i * 2;
                if offset + 2 > data.len() {
                    break;
                }
                let val = i16::from_le_bytes([data[offset], data[offset + 1]]);
                samples.push(val as f32 / i16::MAX as f32);
            }
        }
        Sample::I16(Type::Planar) => {
            for i in 0..nb {
                if samples.len() >= max_samples {
                    break;
                }
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    let data = frame.data(ch);
                    let offset = i * 2;
                    if offset + 2 <= data.len() {
                        let val = i16::from_le_bytes([data[offset], data[offset + 1]]);
                        sum += val as f32 / i16::MAX as f32;
                    }
                }
                samples.push(sum / channels as f32);
            }
        }
        other => log::debug!("波形暂不支持的采样格式: {other:?}"),
    }
}

/// 将峰值重采样到目标条数（线性插值索引）
fn resample_peaks(peaks: &[f32], target_bars: usize) -> Vec<f32> {
    if peaks.is_empty() || target_bars == 0 {
        return Vec::new();
    }
    if peaks.len() == target_bars {
        return peaks.to_vec();
    }
    let mut out = Vec::with_capacity(target_bars);
    for i in 0..target_bars {
        let src_idx = i * peaks.len() / target_bars;
        out.push(peaks[src_idx.min(peaks.len() - 1)]);
    }
    out
}

/// 解码音频并计算每段峰值（0.0~1.0）；限制最多读取约 120s 避免大文件阻塞。
#[cfg(feature = "video")]
fn decode_audio_peaks(path: &Path, bars: usize) -> Option<Vec<f32>> {
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::input;
    use ffmpeg::media::Type;
    use ffmpeg::software::resampling::context::Context as ResampleContext;
    use ffmpeg::util::frame::Audio as AudioFrame;
    use ffmpeg::ChannelLayout;

    ffmpeg::init().ok()?;
    let mut ictx = input(path).ok()?;
    let stream = ictx
        .streams()
        .find(|s| s.parameters().medium() == Type::Audio)
        .or_else(|| ictx.streams().best(Type::Audio));
    let stream = match stream {
        Some(s) => s,
        None => {
            log::debug!("波形无音频流 {:?}", path);
            return None;
        }
    };
    let stream_index = stream.index();
    let codec_params = stream.parameters();

    let context = ffmpeg::codec::context::Context::from_parameters(codec_params).ok()?;
    let mut decoder = match context.decoder().audio() {
        Ok(d) => d,
        Err(e) => {
            log::debug!("波形打开音频解码器失败 {:?}: {e}", path);
            return None;
        }
    };

    // resampler 对部分 ADTS/AAC 流会初始化失败，失败时直接读解码帧原始 PCM
    let mut resampler = ResampleContext::get(
        decoder.format(),
        decoder.channel_layout(),
        decoder.rate(),
        ffmpeg::format::sample::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ChannelLayout::MONO,
        decoder.rate().max(8000),
    )
    .ok();
    if resampler.is_none() {
        log::debug!(
            "波形 resampler 不可用，使用原始 PCM {:?} ({:?})",
            path,
            decoder.format()
        );
    }

    let mut samples: Vec<f32> = Vec::new();
    const MAX_SAMPLES: usize = 8000 * 120;

    let mut decoded = AudioFrame::empty();
    let mut resampled = AudioFrame::empty();

    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if let Err(e) = decoder.send_packet(&packet) {
            log::debug!("波形 send_packet 失败 {:?}: {e}", path);
            continue;
        }
        if drain_decoder_frames(
            path,
            &mut decoder,
            &mut resampler,
            &mut decoded,
            &mut resampled,
            &mut samples,
            MAX_SAMPLES,
        ) {
            break;
        }
    }

    // 短文件 / ADTS 等需在 EOF 后排空解码器缓冲
    if let Err(e) = decoder.send_eof() {
        log::debug!("波形 send_eof 失败 {:?}: {e}", path);
    } else {
        let _ = drain_decoder_frames(
            path,
            &mut decoder,
            &mut resampler,
            &mut decoded,
            &mut resampled,
            &mut samples,
            MAX_SAMPLES,
        );
    }

    if samples.is_empty() {
        log::debug!("波形解码无采样 {:?} (bars={bars})", path);
        return None;
    }

    let chunk = (samples.len() / bars).max(1);
    let mut peaks = Vec::with_capacity(bars);
    for i in 0..bars {
        let start = i * chunk;
        let end = (start + chunk).min(samples.len());
        if start >= samples.len() {
            peaks.push(0.0);
            continue;
        }
        let max = samples[start..end]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        peaks.push(max);
    }
    let peak_max = peaks.iter().copied().fold(0.0f32, f32::max);
    if peak_max > f32::EPSILON {
        for p in &mut peaks {
            *p /= peak_max;
        }
    }
    Some(peaks)
}

/// 透明底 + 1px 细条对称波形（固定柱宽，条数由画布宽度与 peaks 共同决定）
pub fn draw_waveform(peaks: &[f32], width: u32, height: u32, style: WaveformStyle) -> RgbaImage {
    draw_waveform_bars(peaks, width, height, style)
}

/// 固定 1px 柱宽绘制；峰值不足时重采样以填满画布宽度
pub fn draw_waveform_bars(peaks: &[f32], width: u32, height: u32, style: WaveformStyle) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(width, height, style.background);
    if peaks.is_empty() || width == 0 || height == 0 {
        return img;
    }

    let step = style.bar_width + style.bar_gap;
    let target_bars = ((width + style.bar_gap) / step).max(1) as usize;
    let bars = resample_peaks(peaks, target_bars);
    if bars.is_empty() {
        return img;
    }

    let mid = height / 2;
    let usable_h = (height as f32 - 4.0).max(1.0);

    for (i, &peak) in bars.iter().enumerate() {
        if peak <= f32::EPSILON {
            continue;
        }
        let x = (i as u32).saturating_mul(step);
        if x >= width {
            break;
        }
        let bar_h = (usable_h * peak * 0.92).round().max(1.0) as u32;
        let y0 = mid.saturating_sub(bar_h / 2);
        let y1 = (y0 + bar_h).min(height);
        for y in y0..y1 {
            for dx in 0..style.bar_width.min(width.saturating_sub(x)) {
                img.put_pixel(x + dx, y, style.bar);
            }
        }
    }
    img
}

#[cfg(not(feature = "video"))]
pub fn render_audio_waveform(
    _path: &Path,
    _width: u32,
    _height: u32,
    _bars: usize,
) -> Option<RgbaImage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_waveform_produces_image() {
        let peaks: Vec<f32> = (0..64).map(|i| (i as f32 / 64.0).sin().abs()).collect();
        let img = draw_waveform(&peaks, 256, 128, WaveformStyle::default());
        assert_eq!(img.width(), 256);
        assert_eq!(img.height(), 128);
    }

    #[test]
    fn thin_bars_use_one_pixel_width() {
        let peaks = vec![1.0f32; 256];
        let img = draw_waveform_bars(&peaks, 512, 128, WaveformStyle::default());
        // 256 条 × (1px 柱 + 1px 间距) 应落在 512 宽内
        assert!(img.width() >= 256);
        let purple = BAR_COLOR;
        let mut colored_x = 0u32;
        for x in 0..img.width() {
            if img.get_pixel(x, 64) == &purple {
                colored_x = x;
                break;
            }
        }
        assert_eq!(colored_x % 2, 0);
    }

    #[test]
    fn resample_peaks_expands_to_target() {
        let peaks = vec![0.5, 1.0, 0.25];
        let out = resample_peaks(&peaks, 6);
        assert_eq!(out.len(), 6);
    }
}
