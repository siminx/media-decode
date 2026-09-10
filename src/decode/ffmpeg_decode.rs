//! FFmpeg 首帧解码：video-rs（J2K 图像）+ ffmpeg-next（视频/SWF，带 packet 预算）。

use std::collections::HashMap;
use std::path::Path;

use image::{DynamicImage, ImageBuffer, Rgb};
use thiserror::Error;

use crate::frame_valid::is_effectively_blank;
use super::{ffmpeg_log, ffmpeg_probe};

/// 视频首帧解码失败原因（供损坏容器早停判定）
#[derive(Error, Debug)]
pub enum VideoFrameError {
    #[error("无法打开视频: {0}")]
    OpenFailed(String),
    #[error("StreamNotFound")]
    NoVideoStream,
    #[error("视频尺寸无效")]
    InvalidSize,
    #[error("packet budget exhausted")]
    PacketBudgetExhausted,
    #[error("consecutive decode failures")]
    ConsecutiveDecodeFailures,
    #[error("no frame decoded")]
    NoFrame,
}

/// 最多读取的视频 packet 数（损坏流早停）
const MAX_VIDEO_PACKETS: u32 = 48;
/// seek 后放宽 packet 预算，等待 keyframe 后有效帧
const MAX_VIDEO_PACKETS_AFTER_SEEK: u32 = 64;
/// 连续无帧 decode 次数上限
const MAX_DECODE_FAILS: u32 = 8;
/// 同一 seek 点内最多跳过的 blank 帧数
const MAX_BLANK_FRAMES: u32 = 8;

/// 从媒体/图像文件解码第一帧为 RGB8
#[cfg(feature = "video")]
pub fn decode_first_frame(path: &Path) -> Option<DynamicImage> {
    ffmpeg_log::init_ffmpeg_logging();
    if is_swf(path) {
        return decode_swf_first_frame(path);
    }
    decode_first_frame_with_probe(path, ffmpeg_probe::probe_options_for_path(path))
}

/// 使用指定探针参数解码首帧（J2K 临时文件等场景可传入字节长度推导的 probesize）
#[cfg(feature = "video")]
pub fn decode_first_frame_with_probe(
    path: &Path,
    probe: HashMap<String, String>,
) -> Option<DynamicImage> {
    ffmpeg_log::init_ffmpeg_logging();
    use video_rs::decode::DecoderBuilder;
    use video_rs::Options;

    let opts: Options = probe.into();
    let mut decoder = DecoderBuilder::new(path)
        .with_options(&opts)
        .build()
        .ok()?;

    let (w, h) = decoder.size();
    if w == 0 || h == 0 {
        return None;
    }

    let frame = decoder.decode().ok()?.1;
    frame_to_rgb8(w, h, frame.as_slice()?)
}

/// ffmpeg-next 解码视频首帧（带 packet 预算，忽略音频流）
#[cfg(feature = "video")]
pub fn decode_video_first_frame(
    path: &Path,
    seek_secs: f64,
    probe: HashMap<String, String>,
) -> Result<DynamicImage, VideoFrameError> {
    ffmpeg_log::init_ffmpeg_logging();
    decode_video_first_frame_inner(path, seek_secs, Some(probe))
}

/// 探针视频时长（秒）；容器无 duration 元数据时返回 None
#[cfg(feature = "video")]
pub fn probe_duration_secs(path: &Path, probe: HashMap<String, String>) -> Option<f64> {
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::input_with_dictionary;
    use ffmpeg::Dictionary;

    ffmpeg_log::init_ffmpeg_logging();
    ffmpeg::init().ok()?;
    let dict: Dictionary = probe.into_iter().collect();
    let ictx = input_with_dictionary(path, dict).ok()?;
    let duration = ictx.duration();
    if duration <= 0 {
        return None;
    }
    Some(duration as f64 / ffmpeg::ffi::AV_TIME_BASE as f64)
}

/// 探测视频宽高与时长：一次容器打开同时取两者，避免探测方打开两次。
/// 宽高直接从解码器参数读（不实际解码帧），无视频流或尺寸无效时为 None。
#[cfg(feature = "video")]
pub fn probe_video_meta(
    path: &Path,
    probe: HashMap<String, String>,
) -> (Option<(u32, u32)>, Option<f64>) {
    use ffmpeg::Dictionary;
    use ffmpeg::format::input_with_dictionary;
    use ffmpeg::media::Type;
    use ffmpeg_next as ffmpeg;

    ffmpeg_log::init_ffmpeg_logging();
    if ffmpeg::init().is_err() {
        return (None, None);
    }
    let dict: Dictionary = probe.into_iter().collect();
    let Ok(ictx) = input_with_dictionary(path, dict) else {
        return (None, None);
    };

    let duration_secs = {
        let duration = ictx.duration();
        (duration > 0).then(|| duration as f64 / ffmpeg::ffi::AV_TIME_BASE as f64)
    };
    let dimensions = ictx
        .streams()
        .best(Type::Video)
        .and_then(|stream| {
            ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .ok()?
                .decoder()
                .video()
                .ok()
                .map(|decoder| (decoder.width(), decoder.height()))
        })
        .filter(|(w, h)| *w > 0 && *h > 0);
    (dimensions, duration_secs)
}

/// SWF：ffmpeg-next 只取首帧并 swscale 到 RGB24
#[cfg(feature = "video")]
pub fn decode_swf_first_frame(path: &Path) -> Option<DynamicImage> {
    decode_video_first_frame(path, 0.0, HashMap::new()).ok()
}

/// 均匀抽取视频帧（视频语义索引）：在时长上均匀取最多 `max_frames` 个 seek 点解码。
/// 解码失败的时间点跳过；全部失败时回退首帧。供 spike 与闲时 CLIP 流水线复用。
#[cfg(feature = "video")]
pub fn decode_video_sample_frames(path: &Path, max_frames: usize) -> Vec<DynamicImage> {
    ffmpeg_log::init_ffmpeg_logging();
    let probe = ffmpeg_probe::probe_options_for_path(path);
    let duration = probe_duration_secs(path, probe.clone()).unwrap_or(0.0);
    let count = max_frames.clamp(1, 5);
    let seek_points: Vec<f64> = if duration <= 0.5 {
        vec![0.0]
    } else {
        (0..count)
            .map(|i| duration * i as f64 / count as f64)
            .collect()
    };
    let mut frames = Vec::new();
    for secs in seek_points {
        if let Ok(frame) = decode_video_first_frame(path, secs, probe.clone()) {
            frames.push(frame);
        }
    }
    if frames.is_empty() {
        if let Ok(frame) = decode_video_first_frame(path, 0.0, probe) {
            frames.push(frame);
        }
    }
    frames
}

#[cfg(feature = "video")]
fn decode_video_first_frame_inner(
    path: &Path,
    seek_secs: f64,
    probe: Option<HashMap<String, String>>,
) -> Result<DynamicImage, VideoFrameError> {
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::input_with_dictionary;
    use ffmpeg::media::Type;
    use ffmpeg::software::scaling::{context::Context, flag::Flags};
    use ffmpeg::util::frame::video::Video;
    use ffmpeg::Dictionary;

    ffmpeg::init().map_err(|e| VideoFrameError::OpenFailed(e.to_string()))?;

    let dict: Dictionary = probe
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut ictx = input_with_dictionary(path, dict)
        .map_err(|e| VideoFrameError::OpenFailed(format!("{e:?}")))?;

    let input_stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or(VideoFrameError::NoVideoStream)?;
    let stream_index = input_stream.index();

    let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
        .map_err(|e| VideoFrameError::OpenFailed(e.to_string()))?;
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|e| VideoFrameError::OpenFailed(e.to_string()))?;

    let (dec_w, dec_h) = (decoder.width(), decoder.height());
    if dec_w == 0 || dec_h == 0 {
        return Err(VideoFrameError::InvalidSize);
    }

    if seek_secs > 0.0 {
        let ts = (seek_secs * ffmpeg::ffi::AV_TIME_BASE as f64).round() as i64;
        let flags = ffmpeg::ffi::AVSEEK_FLAG_BACKWARD;
        let ret = unsafe {
            ffmpeg::ffi::avformat_seek_file(
                ictx.as_mut_ptr(),
                -1,
                i64::MIN,
                ts,
                ts,
                flags,
            )
        };
        if ret < 0 {
            log::debug!("seek 失败 seek_secs={seek_secs} path={path:?}");
        }
        decoder.flush();
    }

    let max_packets = if seek_secs > 0.0 {
        MAX_VIDEO_PACKETS_AFTER_SEEK
    } else {
        MAX_VIDEO_PACKETS
    };

    let mut scaler = Context::get(
        decoder.format(),
        dec_w,
        dec_h,
        ffmpeg::format::pixel::Pixel::RGB24,
        dec_w,
        dec_h,
        Flags::BILINEAR,
    )
    .map_err(|e| VideoFrameError::OpenFailed(e.to_string()))?;

    let mut decoded = Video::empty();
    let mut rgb_frame = Video::empty();
    let mut video_packets = 0u32;
    let mut consecutive_fails = 0u32;
    let mut blank_frames = 0u32;
    let mut blank_fallback: Option<DynamicImage> = None;

    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_index {
            continue;
        }

        video_packets += 1;
        if video_packets > max_packets {
            return Err(VideoFrameError::PacketBudgetExhausted);
        }

        if decoder.send_packet(&packet).is_err() {
            consecutive_fails += 1;
            if consecutive_fails >= MAX_DECODE_FAILS {
                return Err(VideoFrameError::ConsecutiveDecodeFailures);
            }
            continue;
        }

        let mut got_frame = false;
        while decoder.receive_frame(&mut decoded).is_ok() {
            got_frame = true;
            consecutive_fails = 0;
            scaler
                .run(&decoded, &mut rgb_frame)
                .map_err(|e| VideoFrameError::OpenFailed(e.to_string()))?;
            if let Some(img) = rgb_video_to_dynamic(&rgb_frame) {
                if is_effectively_blank(&img) {
                    blank_fallback = Some(img);
                    blank_frames += 1;
                    if blank_frames < MAX_BLANK_FRAMES {
                        continue;
                    }
                    return Ok(blank_fallback.take().expect("blank fallback"));
                }
                return Ok(img);
            }
        }

        if !got_frame {
            consecutive_fails += 1;
            if consecutive_fails >= MAX_DECODE_FAILS {
                return Err(VideoFrameError::ConsecutiveDecodeFailures);
            }
        }

    }

    if let Some(img) = blank_fallback {
        return Ok(img);
    }

    Err(VideoFrameError::NoFrame)
}

#[cfg(feature = "video")]
fn rgb_video_to_dynamic(frame: &ffmpeg_next::util::frame::video::Video) -> Option<DynamicImage> {
    let w = frame.width();
    let h = frame.height();
    let data = frame.data(0);
    let stride = frame.stride(0);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for row in 0..h as usize {
        let start = row * stride;
        pixels.extend_from_slice(&data[start..start + (w as usize) * 3]);
    }
    ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(w, h, pixels).map(DynamicImage::ImageRgb8)
}

#[cfg(feature = "video")]
fn is_swf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("swf"))
}

#[cfg(feature = "video")]
fn frame_to_rgb8(w: u32, h: u32, buf: &[u8]) -> Option<DynamicImage> {
    let expected = (w as usize).checked_mul(h as usize)?.checked_mul(3)?;
    if buf.len() < expected {
        return None;
    }
    let rgb = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(w, h, buf[..expected].to_vec())?;
    Some(DynamicImage::ImageRgb8(rgb))
}

#[cfg(not(feature = "video"))]
pub fn decode_first_frame(_path: &Path) -> Option<DynamicImage> {
    None
}

#[cfg(not(feature = "video"))]
pub fn decode_first_frame_with_probe(
    _path: &Path,
    _probe: HashMap<String, String>,
) -> Option<DynamicImage> {
    None
}

#[cfg(not(feature = "video"))]
pub fn probe_duration_secs(_path: &Path, _probe: HashMap<String, String>) -> Option<f64> {
    None
}

#[cfg(not(feature = "video"))]
pub fn decode_swf_first_frame(_path: &Path) -> Option<DynamicImage> {
    None
}

#[cfg(not(feature = "video"))]
pub fn decode_video_sample_frames(_path: &Path, _max_frames: usize) -> Vec<DynamicImage> {
    Vec::new()
}

#[cfg(not(feature = "video"))]
pub fn decode_video_first_frame(
    _path: &Path,
    _seek_secs: f64,
    _probe: HashMap<String, String>,
) -> Result<DynamicImage, VideoFrameError> {
    Err(VideoFrameError::NoFrame)
}

/// ffmpeg-next 直接解码首帧（J2K 等 video-rs 不稳定的格式）
#[cfg(feature = "video")]
pub fn decode_media_first_frame_ffmpeg_next(path: &Path) -> Option<DynamicImage> {
    decode_swf_first_frame(path)
}

#[cfg(not(feature = "video"))]
pub fn decode_media_first_frame_ffmpeg_next(_path: &Path) -> Option<DynamicImage> {
    None
}
