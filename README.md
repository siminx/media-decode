# media-decode

Multi-format **media decoding** for Rust: rasterize or decode files to pixels, probe metadata, extract PDF text, sample video frames, and generate thumbnail files.

Formerly published as `auto-thumbnail` (0.2.x). Use `media-decode` for new projects.

## Installation

```toml
[dependencies]
media-decode = { version = "0.3", features = ["full"] }
```

### Feature flags

| Feature | Enables |
| ------- | ------- |
| `image` | Raster images via enhanced decode path |
| `video` | FFmpeg video frame extraction + duration probe |
| `pdf` | pdfium render + per-page text extraction |
| `svg` | resvg vector rasterization |
| `raw` | rawler RAW develop (+ X3F embedded JPEG) |
| `audio` | lofty embedded cover art + duration probe |
| `office` | ZIP embedded thumbnail from OOXML/ODF |
| `source` | PSD/AI design source previews |
| `full` | All of the above (default) |

## API overview

### Thumbnails (write image file)

```rust
use media_decode::Thumbnailer;

let thumbnailer = Thumbnailer::default();
thumbnailer.create_thumbnail("demo/1.webp", "demo/output.webp")?;
```

### Decode to pixels (no file output)

Shared by thumbnails, similarity search, palette extraction, and previews:

```rust
use media_decode::{decode_image, decode_and_thumbnail, decode_for_thumbnail};

let img = decode_image("photo.hdr")?;
let thumb = decode_and_thumbnail("logo.ico", 256)?;
let routed = decode_for_thumbnail("icon.svg", 512)?;
```

### Media metadata (no full decode)

```rust
use media_decode::probe_media_meta;

if let Some(meta) = probe_media_meta(path) {
    println!("{:?} x {:?}, duration {:?}", meta.width, meta.height, meta.duration_secs);
}
```

### PDF text and multi-page render

```rust
#[cfg(feature = "pdf")]
{
    use media_decode::{extract_pdf_pages_text, render_pdf_pages_for_ocr};
    let texts = extract_pdf_pages_text(path, 32);
    let pages = render_pdf_pages_for_ocr(path, 8, 2.0);
}
```

### Video frame sampling

```rust
#[cfg(feature = "video")]
{
    use media_decode::decode_video_sample_frames;
    let frames = decode_video_sample_frames(path, 4);
}
```

### Blank-frame detection

```rust
use media_decode::{is_effectively_blank, is_rgba_blank};
```

## Building

### Video (`feature = "video"`)

Requires **FFmpeg 8.x** dev libraries (`ffmpeg-next 8.1`). See [cherry-box docs](https://github.com/siminx/media-decode) or vcpkg example in project wiki.

### PDF (`feature = "pdf"`)

Requires **pdfium** at runtime. See [pdfium-render](https://github.com/ajrcarey/pdfium-render).

## Migrating from auto-thumbnail

Replace dependency:

```toml
# before
auto-thumbnail = { version = "0.2", features = ["full"] }

# after
media-decode = { version = "0.3", features = ["full"] }
```

Replace imports: `auto_thumbnail::` → `media_decode::`.

`auto-thumbnail` 0.3.0 on crates.io is a deprecated re-export shim only.
