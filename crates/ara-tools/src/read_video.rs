// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Port of fixed OMP596f2da utils/video.ts and read.ts video policy.
//! MIT; full original notices retained in the frozen source manifest.
use crate::read_image::{
    ImageCodecPort, ImageResizeOptions, MAX_IMAGE_INPUT_BYTES, ReadImagePolicyPort, format_dimension_note, resize_image,
};
pub use crate::read_video_port::*;
use ara_agent::ToolOutput;
use ara_ai::{ImageContent, UserBlock};
use ara_rpc::{WireString, WireValue};
use base64::Engine;
use futures::future::join_all;
use regex::Regex;
use serde_json::json;
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq)]
pub enum VideoSelector {
    Frame { frame: f64 },
    Time { seconds: f64, raw: String },
}
#[derive(Clone, Debug, Default)]
pub struct VideoMetadata {
    pub duration_sec: Option<f64>,
    pub video_codec: Option<WireValue>,
    pub audio_codec: Option<WireValue>,
    pub width: Option<WireValue>,
    pub height: Option<WireValue>,
    pub fps: Option<f64>,
    pub format_name: Option<WireValue>,
}
#[derive(Clone, Debug)]
pub struct VideoPng {
    pub data: String,
}
#[derive(Clone, Debug)]
pub struct ContactSheet {
    pub png: VideoPng,
    pub thumbs: usize,
    pub cols: usize,
    pub rows: usize,
}
pub struct ReadVideoRuntime {
    pub host: Arc<dyn ReadVideoHostPort>,
    // The Host shares this Arc for its runtime process. Concurrent cold probes
    // are not single-flight; each completion can publish, like fixed JS.
    passthrough: Mutex<Option<Result<Vec<String>, VideoError>>>,
}
impl ReadVideoRuntime {
    pub fn new(host: Arc<dyn ReadVideoHostPort>) -> Self {
        Self { host, passthrough: Mutex::new(None) }
    }
    async fn frame_passthrough_flag(&self) -> Result<Vec<String>, VideoError> {
        if let Some(flags) = self.passthrough.lock().unwrap().clone() {
            return flags;
        }
        let attempt = async {
            let executable = require_binary(self.host.as_ref(), MediaBinary::Ffmpeg).await?;
            // Fixed version probe has no Run signal; its Host lifetime owner
            // still settles it. A cancelled frame checks only at run_ffmpeg.
            let output = self.host.execute(MediaCommand { executable, args: vec!["-version".into()] }, None).await?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version = stdout.lines().next().unwrap_or("").split("version").nth(1).unwrap_or("");
            let major = js_parse_int(ara_prompt::js::trim(version).split('.').next().unwrap_or(""));
            Ok::<_, VideoError>(if major.is_finite() && major < 5.0 {
                vec!["-vsync".into(), "0".into()]
            } else {
                modern_flag()
            })
        }
        .await;
        let flags = match attempt {
            Ok(flags) => Ok(flags),
            Err(error) if error.is_unknown() || error.is_aborted() => Err(error),
            Err(_) => Ok(modern_flag()), // Keep the fixed confirmed-error fallback.
        };
        let mut cache = self.passthrough.lock().unwrap();
        // Original concurrent cold probes still run independently. Once an
        // uncertain/aborted probe is observed, a late ordinary completion must
        // not clear it or permit a later automatic probe replay in this runtime.
        let incoming_unknown = flags.as_ref().err().is_some_and(VideoError::is_unknown);
        if let Some(Err(error)) = &*cache
            && (error.is_unknown() || error.is_aborted() && !incoming_unknown)
        {
            return Err(error.clone());
        }
        *cache = Some(flags.clone());
        flags
    }
}
fn modern_flag() -> Vec<String> {
    vec!["-fps_mode".into(), "passthrough".into()]
}
fn expression<'a>(cell: &'a OnceLock<Regex>, pattern: &str) -> &'a Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}
pub fn video_mime_for_path(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "m4v" => "video/x-m4v",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        _ => return None,
    })
}
pub fn is_video_path(path: &str) -> bool {
    video_mime_for_path(path).is_some()
}
pub fn parse_video_selector(selector: Option<&str>) -> Option<VideoSelector> {
    let selector = selector.filter(|s| !s.is_empty())?;
    let trimmed = ara_prompt::js::trim(selector);
    static FRAME: OnceLock<Regex> = OnceLock::new();
    if let Some(capture) = expression(&FRAME, r"(?i)^(?:f|frame)?([0-9]+)$").captures(trimmed) {
        let frame = js_parse_int(&capture[1]);
        return (frame.is_finite() && frame <= 9_007_199_254_740_991.0).then_some(VideoSelector::Frame { frame });
    }
    parse_video_timestamp(trimmed).map(|seconds| VideoSelector::Time { seconds, raw: trimmed.into() })
}
pub fn parse_video_timestamp(value: &str) -> Option<f64> {
    static COLON: OnceLock<Regex> = OnceLock::new();
    static UNIT: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    if let Some(capture) =
        expression(&COLON, r"^(?:([0-9]+):)?([0-5]?[0-9]):([0-5]?[0-9](?:\.[0-9]+)?)$").captures(value)
    {
        let hours = capture.get(1).map(|m| js_parse_int(m.as_str())).unwrap_or(0.0);
        return Some(hours * 3600.0 + js_parse_int(&capture[2]) * 60.0 + js_parse_float(&capture[3]));
    }
    if let Some(capture) =
        expression(&UNIT, r"(?i)^(?:([0-9]+(?:\.[0-9]+)?)h)?(?:([0-9]+(?:\.[0-9]+)?)m)?(?:([0-9]+(?:\.[0-9]+)?)s)?$")
            .captures(value)
        && (1..=3).any(|i| capture.get(i).is_some())
    {
        let number = |i| capture.get(i).map(|m| js_parse_float(m.as_str())).unwrap_or(0.0);
        return Some(number(1) * 3600.0 + number(2) * 60.0 + number(3));
    }
    expression(&BARE, r"^[0-9]+\.[0-9]+$").is_match(value).then(|| js_parse_float(value))
}
pub fn split_video_read_target(path: &str) -> Option<(String, String)> {
    static TARGET: OnceLock<Regex> = OnceLock::new();
    let captures = expression(&TARGET, r"(?is)^(.*\.(?:mp4|mov|mkv|webm|m4v|avi|wmv)):(.+)$").captures(path)?;
    (is_video_path(&captures[1]) && parse_video_selector(Some(&captures[2])).is_some())
        .then(|| (captures[1].into(), captures[2].into()))
}
fn js_parse_float(value: &str) -> f64 {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    let Some(prefix) = expression(&PREFIX, r"^[+-]?(?:Infinity|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)")
        .find(ara_prompt::js::trim_start(value))
    else {
        return f64::NAN;
    };
    match prefix.as_str() {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        n => n.parse().unwrap_or(f64::NAN),
    }
}
fn js_parse_int(value: &str) -> f64 {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    expression(&PREFIX, r"^[+-]?[0-9]+")
        .find(ara_prompt::js::trim_start(value))
        .map(|n| n.as_str().parse().unwrap_or(f64::NAN))
        .unwrap_or(f64::NAN)
}
fn wire_string(value: &WireValue) -> String {
    match value {
        WireValue::String(s) => String::from_utf16_lossy(s.units()),
        WireValue::Number(n) => ara_prompt::js::f64_to_string(*n),
        WireValue::Null => "null".into(),
        WireValue::Bool(b) => b.to_string(),
        WireValue::Object(_) => "[object Object]".into(),
        WireValue::Array(items) => items
            .iter()
            .map(|item| if matches!(item, WireValue::Null) { String::new() } else { wire_string(item) })
            .collect::<Vec<_>>()
            .join(","),
    }
}
fn truthy(value: &WireValue) -> bool {
    match value {
        WireValue::Null => false,
        WireValue::Bool(b) => *b,
        WireValue::Number(n) => *n != 0.0 && !n.is_nan(),
        WireValue::String(s) => !s.is_empty(),
        _ => true,
    }
}
fn field<'a>(value: Option<&'a WireValue>, name: &str) -> Option<&'a WireValue> {
    value.and_then(|value| value.get(name))
}
fn frame_rate(value: Option<&WireValue>) -> Result<Option<f64>, VideoError> {
    let Some(value) = value.filter(|value| truthy(value)) else {
        return Ok(None);
    };
    let WireValue::String(value) = value else {
        return Err(VideoError::text("value.indexOf is not a function"));
    };
    let value = value.to_utf8().map_err(|_| VideoError::text("Video metadata requires the native UTF16 boundary"))?;
    if value == "0/0" {
        return Ok(None);
    }
    if let Some((numerator, denominator)) = value.split_once('/') {
        let num = js_parse_float(numerator);
        let den = js_parse_float(denominator);
        return Ok((num.is_finite() && den.is_finite() && den != 0.0 && num > 0.0).then_some(num / den));
    }
    let parsed = js_parse_float(&value);
    Ok((parsed.is_finite() && parsed > 0.0).then_some(parsed))
}
fn find_stream<'a>(streams: &'a [WireValue], kind: &str) -> Result<Option<&'a WireValue>, VideoError> {
    for stream in streams {
        if matches!(stream, WireValue::Null) {
            return Err(VideoError::text("Cannot read properties of null (reading 'codec_type')"));
        }
        if stream
            .get("codec_type")
            .and_then(WireValue::as_string)
            .is_some_and(|value| value.to_utf8().is_ok_and(|value| value == kind))
        {
            return Ok(Some(stream));
        }
    }
    Ok(None)
}
fn require_utf8_metadata(value: &WireValue) -> Result<(), VideoError> {
    match value {
        WireValue::String(value) if value.to_utf8().is_err() => {
            Err(VideoError::text("Video metadata requires the native UTF16 boundary"))
        }
        WireValue::Array(items) => {
            for item in items {
                require_utf8_metadata(item)?;
            }
            Ok(())
        }
        WireValue::Object(items) => {
            for (_, item) in items {
                require_utf8_metadata(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn aborted(cancel: &CancellationToken) -> Result<(), VideoError> {
    if cancel.is_cancelled() { Err(VideoError::aborted("Video operation aborted.")) } else { Ok(()) }
}
async fn require_binary(host: &dyn ReadVideoHostPort, binary: MediaBinary) -> Result<std::path::PathBuf, VideoError> {
    host.find_binary(binary).await?.ok_or_else(||VideoError::text(format!("Reading video requires {}, which was not found on PATH. Install it (e.g. `brew install ffmpeg`) and retry.",binary.name())))
}
fn text_units(bytes: &[u8]) -> Vec<u16> {
    String::from_utf8_lossy(bytes).encode_utf16().collect()
}
fn stderr_tail(bytes: &[u8]) -> Vec<u16> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = ara_prompt::js::trim(&text);
    let units: Vec<_> = trimmed.encode_utf16().collect();
    units[units.len().saturating_sub(300)..].to_vec()
}
fn wire_error(prefix: &str, tail: Vec<u16>) -> VideoError {
    let mut units: Vec<_> = prefix.encode_utf16().collect();
    units.extend(tail);
    VideoError { message: WireString::from_units(units), kind: VideoErrorKind::Ordinary }
}
pub async fn probe_video(
    runtime: &ReadVideoRuntime,
    path: &Path,
    cancel: CancellationToken,
) -> Result<VideoMetadata, VideoError> {
    let executable = require_binary(runtime.host.as_ref(), MediaBinary::Ffprobe).await?;
    aborted(&cancel)?;
    let args = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "format=duration,format_name:stream=index,codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate"
            .into(),
        "-of".into(),
        "json".into(),
        path.to_string_lossy().into_owned(),
    ];
    let output = runtime.host.execute(MediaCommand { executable, args }, Some(cancel.clone())).await;
    aborted(&cancel)?;
    let output = output?;
    let basename = path.file_name().unwrap_or_default().to_string_lossy();
    if output.exit_code != 0 {
        let tail = stderr_tail(&output.stderr);
        return Err(wire_error(
            &format!("Could not probe video '{basename}': "),
            if tail.is_empty() {
                format!("ffprobe exited {}", output.exit_code).encode_utf16().collect()
            } else {
                tail
            },
        ));
    }
    let parsed = WireValue::parse_units(&text_units(&output.stdout))
        .map_err(|_| VideoError::text(format!("Could not probe video '{basename}': unexpected ffprobe output.")))?;
    // Optional chaining skips null/undefined, but wrong shapes must not become
    // unknown metadata. The native JS TypeError class is a separate throw gate.
    if matches!(parsed, WireValue::Null) {
        return Err(VideoError::text("Cannot read properties of null (reading 'streams')"));
    }
    let streams = match parsed.get("streams") {
        None | Some(WireValue::Null) => &[][..],
        Some(WireValue::Array(items)) => items.as_slice(),
        Some(_) => return Err(VideoError::text("parsed.streams?.find is not a function")),
    };
    require_utf8_metadata(&parsed)?;
    let video = find_stream(streams, "video")?;
    let audio = find_stream(streams, "audio")?;
    let format = parsed.get("format");
    let duration =
        field(format, "duration").map(|v| js_parse_float(&wire_string(v))).filter(|d| d.is_finite() && *d > 0.0);
    Ok(VideoMetadata {
        duration_sec: duration,
        video_codec: field(video, "codec_name").cloned(),
        audio_codec: field(audio, "codec_name").cloned(),
        width: field(video, "width").cloned(),
        height: field(video, "height").cloned(),
        fps: match frame_rate(field(video, "avg_frame_rate"))? {
            Some(value) => Some(value),
            None => frame_rate(field(video, "r_frame_rate"))?,
        },
        format_name: field(format, "format_name").cloned(),
    })
}
fn fixed(value: f64, digits: u32) -> String {
    if !value.is_finite() || value.abs() >= 1e21 {
        return ara_prompt::js::f64_to_string(value);
    }
    let bits = value.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = u128::from(bits & ((1_u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1_u128 << 52 };
    let shift = if exponent == 0 { 1074 } else { 1075 - exponent };
    let scale = 10_u128.pow(digits);
    let scaled = mantissa * scale;
    let n = if shift >= 128 {
        0
    } else if shift > 0 {
        let den = 1_u128 << shift;
        (scaled + den / 2) / den
    } else {
        scaled << (-shift)
    };
    format!("{}{}.{:0width$}", if value < 0.0 { "-" } else { "" }, n / scale, n % scale, width = digits as usize)
}
fn nonnegative_floor(seconds: f64) -> f64 {
    if seconds.is_nan() { f64::NAN } else { seconds.floor().max(0.0) }
}
fn padded(value: f64) -> String {
    let value = ara_prompt::js::f64_to_string(value);
    if value.len() < 2 { format!("0{value}") } else { value }
}
pub fn format_video_timestamp(seconds: f64) -> String {
    let floored = nonnegative_floor(seconds);
    let hours = (floored / 3600.0).floor();
    let minutes = ((floored % 3600.0) / 60.0).floor();
    let seconds = floored % 60.0;
    if hours > 0.0 {
        format!(
            "{}h{}m{}s",
            ara_prompt::js::f64_to_string(hours),
            ara_prompt::js::f64_to_string(minutes),
            padded(seconds)
        )
    } else {
        format!("{}:{}", ara_prompt::js::f64_to_string(minutes), padded(seconds))
    }
}
fn duration_long(seconds: f64) -> String {
    let floored = nonnegative_floor(seconds);
    let hours = (floored / 3600.0).floor();
    let minutes = ((floored % 3600.0) / 60.0).floor();
    let seconds = floored % 60.0;
    let mut parts = Vec::new();
    if hours > 0.0 {
        parts.push(format!("{}h", ara_prompt::js::f64_to_string(hours)));
    }
    if minutes > 0.0 || hours > 0.0 {
        parts.push(format!("{}m", ara_prompt::js::f64_to_string(minutes)));
    }
    parts.push(format!("{}s", ara_prompt::js::f64_to_string(seconds)));
    parts.join(" ")
}
fn byte_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes < 1024.0 {
        format!("{} B", ara_prompt::js::f64_to_string(bytes))
    } else if bytes / 1024.0 < 1024.0 {
        format!("{} KiB", fixed(bytes / 1024.0, 1))
    } else if bytes / (1024.0 * 1024.0) < 1024.0 {
        format!("{} MiB", fixed(bytes / (1024.0 * 1024.0), 1))
    } else {
        format!("{} GiB", fixed(bytes / (1024.0 * 1024.0 * 1024.0), 1))
    }
}
pub fn format_video_details(path: &str, meta: &VideoMetadata, file_size: u64, mime: Option<&str>) -> String {
    let mut lines = vec![format!("Video: {path}")];
    let format = meta.format_name.as_ref().filter(|f| truthy(f)).map(wire_string);
    if let Some(mime) = mime {
        lines.push(format!("- Container: {mime}{}", format.as_ref().map(|f| format!(" ({f})")).unwrap_or_default()));
    } else if let Some(format) = format {
        lines.push(format!("- Container: {format}"));
    }
    let codec = |value: &Option<WireValue>, fallback: &str| {
        value.as_ref().filter(|v| !matches!(v, WireValue::Null)).map(wire_string).unwrap_or_else(|| fallback.into())
    };
    lines.extend([
        format!(
            "- Duration: {}",
            meta.duration_sec
                .map(|d| format!("{} ({}s)", duration_long(d), fixed(d, 1)))
                .unwrap_or_else(|| "unknown".into())
        ),
        format!(
            "- Resolution: {}",
            meta.width
                .as_ref()
                .zip(meta.height.as_ref())
                .map(|(w, h)| format!("{}x{}", wire_string(w), wire_string(h)))
                .unwrap_or_else(|| "unknown".into())
        ),
        format!("- FPS: {}", meta.fps.map(|fps| fixed(fps, 2)).unwrap_or_else(|| "unknown".into())),
        format!("- Video codec: {}", codec(&meta.video_codec, "unknown")),
        format!("- Audio codec: {}", codec(&meta.audio_codec, "none")),
        format!("- Size: {}", byte_size(file_size)),
    ]);
    lines.join("\n")
}
async fn run_ffmpeg(
    runtime: &ReadVideoRuntime,
    args: Vec<String>,
    cancel: CancellationToken,
) -> Result<(), VideoError> {
    let executable = require_binary(runtime.host.as_ref(), MediaBinary::Ffmpeg).await?;
    aborted(&cancel)?;
    let mut command_args = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    command_args.extend(args);
    let output = runtime.host.execute(MediaCommand { executable, args: command_args }, Some(cancel.clone())).await;
    aborted(&cancel)?;
    let output = output?;
    if output.exit_code != 0 {
        Err(wire_error(&format!("ffmpeg failed (exit {}): ", output.exit_code), stderr_tail(&output.stderr)))
    } else {
        Ok(())
    }
}
fn png(bytes: Vec<u8>) -> VideoPng {
    VideoPng { data: base64::engine::general_purpose::STANDARD.encode(bytes) }
}
pub async fn extract_video_frame_png(
    runtime: &ReadVideoRuntime,
    path: &Path,
    selector: &VideoSelector,
    cancel: CancellationToken,
) -> Result<VideoPng, VideoError> {
    let workspace = runtime.host.create_workspace("omp-video-frame-").await?;
    let result = async {
        let out = workspace.path().join("frame.png").to_string_lossy().into_owned();
        let input = path.to_string_lossy().into_owned();
        let args = match selector {
            VideoSelector::Time { seconds, .. } => vec![
                "-ss".into(),
                ara_prompt::js::f64_to_string(*seconds),
                "-i".into(),
                input,
                "-frames:v".into(),
                "1".into(),
                out,
            ],
            VideoSelector::Frame { frame } => {
                let mut args = vec![
                    "-i".into(),
                    input,
                    "-vf".into(),
                    format!("select='eq(n\\,{})'", ara_prompt::js::f64_to_string(*frame)),
                ];
                args.extend(runtime.frame_passthrough_flag().await?);
                args.extend(["-frames:v".into(), "1".into(), out]);
                args
            }
        };
        run_ffmpeg(runtime, args, cancel.clone()).await?;
        let bytes = workspace.read_named("frame.png").await.unwrap_or(None).filter(|b| !b.is_empty());
        let bytes = bytes.ok_or_else(|| {
            VideoError::text(format!(
                "No frame at {} — it may be past the end of the video.",
                match selector {
                    VideoSelector::Time { raw, .. } => format!("timestamp {raw}"),
                    VideoSelector::Frame { frame } => format!("frame {}", ara_prompt::js::f64_to_string(*frame)),
                }
            ))
        })?;
        aborted(&cancel)?;
        Ok(png(bytes))
    }
    .await;
    if let Err(error) = workspace.cleanup().await
        && error.is_unknown()
    {
        // Do not attach success while an output workspace still has unknown
        // pending writers. The Host retains ownership and later cleanup.
        return Err(error);
    }
    result
}
pub async fn build_video_contact_sheet_png(
    runtime: &ReadVideoRuntime,
    path: &Path,
    meta: &VideoMetadata,
    cancel: CancellationToken,
) -> Result<ContactSheet, VideoError> {
    let workspace = runtime.host.create_workspace("omp-video-sheet-").await?;
    let result = async {
        let times: Vec<_> = (0..6)
            .map(|i| meta.duration_sec.filter(|d| *d > 0.0).map(|d| d * (i as f64 + 0.5) / 6.0).unwrap_or(i as f64))
            .collect();
        let names: Vec<_> = (0..6).map(|i| format!("thumb-{i}.png")).collect();
        let pending = times.iter().zip(names.iter()).map(|(time, name)| {
            let args = vec![
                "-ss".into(),
                ara_prompt::js::f64_to_string(*time),
                "-i".into(),
                path.to_string_lossy().into_owned(),
                "-frames:v".into(),
                "1".into(),
                "-vf".into(),
                "scale=320:-1".into(),
                workspace.path().join(name).to_string_lossy().into_owned(),
            ];
            let token = cancel.clone();
            async move {
                let result = run_ffmpeg(runtime, args, token.clone()).await;
                match result {
                    Err(error) if token.is_cancelled() || error.is_unknown() || error.is_aborted() => Err(error),
                    _ => Ok(()), // Fixed missing-frame catch only for settled ordinary faults.
                }
            }
        });
        // Await all six workers even on cancellation so Host cleanup cannot
        // race retained children. Original Promise.all starts all six at once.
        for result in join_all(pending).await {
            result?;
        }
        let mut available = Vec::new();
        for name in names {
            if workspace.nonempty_named(&name).await? {
                available.push(name);
            }
        }
        if available.is_empty() {
            return Err(VideoError::text("Could not extract any preview frames from this video."));
        }
        let mut args = Vec::new();
        for name in &available {
            args.extend(["-i".into(), workspace.path().join(name).to_string_lossy().into_owned()]);
        }
        let concat = (0..available.len()).map(|i| format!("[{i}:v]")).collect::<Vec<_>>().join("");
        args.extend([
            "-filter_complex".into(),
            format!("{concat}concat=n={}:v=1:a=0,tile=3x2", available.len()),
            "-frames:v".into(),
            "1".into(),
            workspace.path().join("sheet.png").to_string_lossy().into_owned(),
        ]);
        run_ffmpeg(runtime, args, cancel.clone()).await?;
        let bytes = workspace
            .read_named("sheet.png")
            .await
            .unwrap_or(None)
            .filter(|b| !b.is_empty())
            .ok_or_else(|| VideoError::text("Could not build a video preview grid for this file."))?;
        aborted(&cancel)?;
        Ok(ContactSheet { png: png(bytes), thumbs: available.len(), cols: 3, rows: 2 })
    }
    .await;
    if let Err(error) = workspace.cleanup().await
        && error.is_unknown()
    {
        // Do not attach success while an output workspace still has unknown
        // pending writers. The Host retains ownership and later cleanup.
        return Err(error);
    }
    result
}
pub async fn read_video_file(
    runtime: &ReadVideoRuntime,
    path: &Path,
    display: &str,
    selector: Option<&str>,
    file_size: u64,
    auto_resize: bool,
    policy: Option<&dyn ReadImagePolicyPort>,
    codec: Option<&dyn ImageCodecPort>,
    cancel: CancellationToken,
) -> Result<ToolOutput, VideoError> {
    let parsed = parse_video_selector(selector);
    if parsed.is_none()
        && let Some(selector) = selector
    {
        return Err(VideoError::text(format!(
            "Invalid selector ':{selector}' on '{display}'. Use :<frame> (e.g. :412) or :<timestamp> (e.g. :1h5m42s, :90s) to extract a frame, or read without a selector for a preview grid."
        )));
    }
    let mime = video_mime_for_path(&path.to_string_lossy());
    let meta = probe_video(runtime, path, cancel.clone()).await?;
    let state = policy.map(|p| p.current_state()).unwrap_or_default();
    let details = format_video_details(display, &meta, file_size, mime);
    if state.inspect_image_active {
        aborted(&cancel)?;
        let hint = if parsed.is_some() {
            String::new()
        } else {
            format!(
                "\n\nIf you want to see the video, read a frame with {display}:<timestamp> (e.g. :0:05) or {display}:<frame> (e.g. :412)."
            )
        };
        return Ok(ToolOutput::text(details+&hint).with_details(json!({"resolvedPath":path.to_string_lossy(),"contentType":mime,"fileSize":file_size,"meta":{"source":{"type":"path","value":path.to_string_lossy()}}})));
    }
    let (png, label) = if let Some(selector) = parsed {
        let label = match &selector {
            VideoSelector::Time { raw, .. } => format!("Video frame: {display} @ {raw}"),
            VideoSelector::Frame { frame } => {
                format!("Video frame: {display} (frame {})", ara_prompt::js::f64_to_string(*frame))
            }
        };
        (extract_video_frame_png(runtime, path, &selector, cancel.clone()).await?, label)
    } else {
        let sheet = build_video_contact_sheet_png(runtime, path, &meta, cancel.clone()).await?;
        (sheet.png, format!("Video preview grid: {display} ({} frames, {}x{})", sheet.thumbs, sheet.cols, sheet.rows))
    };
    let mut image =
        ImageContent { detail: None, compaction_frame: false, data: png.data, mime_type: "image/png".into() };
    let mut dimension = None;
    if auto_resize && let Some(codec) = codec {
        // The fixed model/env policy is read at resize time, after asynchronous
        // ffmpeg extraction. Inspect availability was observed after probing.
        let resize_state = policy.map(|policy| policy.current_state()).unwrap_or_default();
        if let Ok(resized) = resize_image(
            &image.data,
            Some("image/png"),
            &ImageResizeOptions {
                exclude_webp: resize_state.model_excludes_webp.then_some(true),
                ..Default::default()
            },
            resize_state.no_webp_env.as_deref(),
            codec,
            CancellationToken::new(),
        )
        .await
        {
            dimension = format_dimension_note(&resized);
            image.data = resized.data;
            image.mime_type = resized.mime_type;
        }
    }
    aborted(&cancel)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.data)
        .map_err(|e| VideoError::text(e.to_string()))?
        .len() as u64;
    if bytes > MAX_IMAGE_INPUT_BYTES {
        return Err(VideoError::text(format!(
            "Video preview too large: {} exceeds {} limit.",
            crate::read::format_bytes(bytes),
            crate::read::format_bytes(MAX_IMAGE_INPUT_BYTES)
        )));
    }
    let text = format!("{label}\n{details}{}", dimension.map(|note| format!("\n{note}")).unwrap_or_default());
    Ok(ToolOutput {
        content: vec![UserBlock::text(text), UserBlock::Image(image)],
        details: Some(
            json!({"resolvedPath":path.to_string_lossy(),"contentType":mime,"fileSize":file_size,"meta":{"source":{"type":"path","value":path.to_string_lossy()}}}),
        ),
        is_error: false,
        provider_metadata: None,
        useless: false,
    })
}
