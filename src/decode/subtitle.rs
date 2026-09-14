//! 视频字幕轨探测与 VTT 提取：sidecar 由应用层处理，此处负责容器内嵌轨。

use std::path::Path;

/// 容器内一条字幕轨的元信息
#[derive(Clone, Debug)]
pub struct SubtitleStreamInfo {
    pub index: i32,
    pub language: Option<String>,
    pub codec: String,
}

/// 枚举容器内嵌字幕流；无字幕轨时返回空 vec。
#[cfg(feature = "video")]
pub fn list_subtitle_streams(path: &Path) -> Vec<SubtitleStreamInfo> {
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::input_with_dictionary;
    use ffmpeg::media::Type;
    use ffmpeg::Dictionary;

    super::ffmpeg_log::init_ffmpeg_logging();
    if ffmpeg::init().is_err() {
        return Vec::new();
    }
    let probe = super::ffmpeg_probe::probe_options_for_video(path);
    let dict: Dictionary = probe.into_iter().collect();
    let Ok(ictx) = input_with_dictionary(path, dict) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stream in ictx.streams() {
        if stream.parameters().medium() != Type::Subtitle {
            continue;
        }
        let codec = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .ok()
            .map(|ctx| ctx.id().name().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let language = stream
            .metadata()
            .get("language")
            .map(str::to_string);
        out.push(SubtitleStreamInfo {
            index: stream.index() as i32,
            language,
            codec,
        });
    }
    out
}

/// 将指定内嵌字幕轨提取为 WebVTT 文件；成功返回 true。
#[cfg(feature = "video")]
pub fn extract_subtitle_to_vtt(path: &Path, stream_index: i32, out_path: &Path) -> bool {
    use std::process::Command;

    // ffmpeg-next 字幕 mux 到 VTT 链路复杂且版本差异大；复用捆绑 ffmpeg 可执行文件更可靠。
    // 宿主 cherry-box 在 src-tauri/bin/ 放置 ffmpeg.exe，此处优先探测同目录 DLL 旁的 exe。
    let ffmpeg = resolve_ffmpeg_executable();
    let Some(exe) = ffmpeg else {
        log::debug!("未找到 ffmpeg 可执行文件，跳过内嵌字幕提取");
        return false;
    };
    if out_path.exists() {
        let _ = std::fs::remove_file(out_path);
    }
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let status = Command::new(&exe)
        .args([
            "-y",
            "-i",
            &path.to_string_lossy(),
            "-map",
            &format!("0:{stream_index}"),
            "-f",
            "webvtt",
            &out_path.to_string_lossy(),
        ])
        .status();
    match status {
        Ok(s) if s.success() && out_path.is_file() => true,
        Ok(s) => {
            log::debug!("ffmpeg 字幕提取失败 exit={:?} path={path:?}", s.code());
            false
        }
        Err(err) => {
            log::debug!("ffmpeg 字幕提取启动失败: {err}");
            false
        }
    }
}

/// 探测 ffmpeg 可执行文件：环境变量 FFMPEG_PATH → 同进程目录 → PATH。
#[cfg(feature = "video")]
fn resolve_ffmpeg_executable() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("FFMPEG_PATH") {
        let path = std::path::PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["ffmpeg.exe", "ffmpeg"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    which_ffmpeg_in_path()
}

#[cfg(feature = "video")]
fn which_ffmpeg_in_path() -> Option<std::path::PathBuf> {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| std::path::PathBuf::from("ffmpeg"))
}

#[cfg(not(feature = "video"))]
pub fn list_subtitle_streams(_path: &Path) -> Vec<SubtitleStreamInfo> {
    Vec::new()
}

#[cfg(not(feature = "video"))]
pub fn extract_subtitle_to_vtt(_path: &Path, _stream_index: i32, _out_path: &Path) -> bool {
    false
}
