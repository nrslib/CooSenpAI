use anyhow::{Context, Result};
use coosenpai_core::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use objc2::{AnyThread, Message};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext,
    CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
    VNRequestTextRecognitionLevel,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn platform_is_available() -> bool {
    true
}

pub async fn capture_screen(
    destination: PathBuf,
    cancellation: CancellationToken,
) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .context("capture destination に親がありません")?;
    tokio::fs::create_dir_all(parent).await?;
    let runner = TokioProcessRunner;
    let output = runner
        .run(
            ProcessRequest {
                executable: PathBuf::from("/usr/sbin/screencapture"),
                args: vec![
                    "-x".to_owned(),
                    "-m".to_owned(),
                    "-t".to_owned(),
                    "png".to_owned(),
                    destination.display().to_string(),
                ],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(10),
            },
            cancellation,
        )
        .await
        .context("screencapture を起動できません")?;
    if output.status != Some(0) {
        anyhow::bail!("screencapture が失敗しました")
    }
    Ok(destination)
}

pub async fn capture_interactive_region(
    destination: PathBuf,
    cancellation: CancellationToken,
) -> Result<bool> {
    let parent = destination
        .parent()
        .context("capture destination に親がありません")?;
    tokio::fs::create_dir_all(parent).await?;
    let output = TokioProcessRunner
        .run(
            ProcessRequest {
                executable: PathBuf::from("/usr/sbin/screencapture"),
                args: vec![
                    "-i".to_owned(),
                    "-x".to_owned(),
                    destination.display().to_string(),
                ],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(5 * 60),
            },
            cancellation,
        )
        .await?;
    Ok(output.status == Some(0) && destination.is_file())
}

pub fn running_applications() -> Result<Vec<coosenpai_core::ports::RunningApplication>> {
    let applications = NSWorkspace::sharedWorkspace().runningApplications();
    let mut result = applications
        .iter()
        .filter(|application| {
            application.activationPolicy() == NSApplicationActivationPolicy::Regular
        })
        .filter_map(|application| {
            let bundle_id = application.bundleIdentifier()?.to_string();
            let name = application.localizedName()?.to_string();
            if bundle_id == "dev.nrslib.coosenpai" {
                return None;
            }
            let icon_png = application
                .icon()
                .and_then(|icon| icon_png(&icon))
                .unwrap_or_default();
            Some(coosenpai_core::ports::RunningApplication {
                bundle_id,
                name,
                icon_png,
            })
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.bundle_id.cmp(&right.bundle_id))
    });
    result.dedup_by(|left, right| left.bundle_id == right.bundle_id);
    Ok(result)
}

pub fn frontmost_application() -> Option<coosenpai_core::ports::RunningApplication> {
    let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(coosenpai_core::ports::RunningApplication {
        bundle_id: application.bundleIdentifier()?.to_string(),
        name: application.localizedName()?.to_string(),
        icon_png: Vec::new(),
    })
}

pub async fn capture_application_window(
    bundle_id: &str,
    destination: PathBuf,
    cancellation: CancellationToken,
) -> Result<Option<coosenpai_core::ports::ApplicationCapture>> {
    let Some(window) = crate::window_info::application_window(bundle_id)? else {
        return Ok(None);
    };
    let parent = destination
        .parent()
        .context("application capture destination に親がありません")?;
    tokio::fs::create_dir_all(parent).await?;
    let output = TokioProcessRunner
        .run(
            ProcessRequest {
                executable: PathBuf::from("/usr/sbin/screencapture"),
                args: vec![
                    "-l".to_owned(),
                    window.id.to_string(),
                    "-x".to_owned(),
                    "-o".to_owned(),
                    "-t".to_owned(),
                    "png".to_owned(),
                    destination.display().to_string(),
                ],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(10),
            },
            cancellation,
        )
        .await?;
    if output.status != Some(0) {
        anyhow::bail!("アプリウィンドウの screencapture が失敗しました")
    }
    Ok(Some(coosenpai_core::ports::ApplicationCapture {
        path: destination,
        window_id: window.id,
    }))
}

fn icon_png(image: &NSImage) -> Option<Vec<u8>> {
    let tiff = image.TIFFRepresentation()?;
    let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff)?;
    let properties = NSDictionary::new();
    // SAFETY: an empty property dictionary is valid for PNG encoding and its key/value
    // generic parameters match AppKit's declared contract.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }?;
    let length = png.length();
    let mut bytes = vec![0_u8; length];
    let pointer = std::ptr::NonNull::new(bytes.as_mut_ptr().cast())?;
    // SAFETY: `bytes` owns a writable allocation of exactly `length` bytes.
    unsafe { png.getBytes_length(pointer, length) };
    Some(bytes)
}

pub async fn read_hid_idle_ms() -> Result<f64> {
    let output = run_system_command(
        "/usr/sbin/ioreg",
        &["-c", "IOHIDSystem", "-d", "4"],
        Duration::from_secs(2),
    )
    .await?;
    parse_hid_idle_ms(&output)
}

pub async fn read_front_app_name() -> Result<String> {
    Ok(read_front_application().await?.name)
}

pub async fn read_front_application() -> Result<coosenpai_core::ports::RunningApplication> {
    let asn = run_system_command("/usr/bin/lsappinfo", &["front"], Duration::from_secs(2)).await?;
    let asn = parse_front_app_asn(&asn)?;
    let info = run_system_command(
        "/usr/bin/lsappinfo",
        &["info", "-only", "name", "-only", "bundleid", &asn],
        Duration::from_secs(2),
    )
    .await?;
    Ok(coosenpai_core::ports::RunningApplication {
        name: parse_quoted_field(&info, "LSDisplayName")?,
        bundle_id: parse_quoted_field(&info, "CFBundleIdentifier")?,
        icon_png: Vec::new(),
    })
}

static SCREEN_CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn screen_capture_permission() -> coosenpai_core::ports::ScreenCapturePermission {
    coosenpai_core::ports::ScreenCapturePermission::from_preflight(
        objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        SCREEN_CAPTURE_REQUESTED.load(Ordering::Acquire),
    )
}

pub fn request_screen_capture_permission() -> coosenpai_core::ports::ScreenCapturePermission {
    if objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        return coosenpai_core::ports::ScreenCapturePermission::from_preflight(true, true);
    }
    if SCREEN_CAPTURE_REQUESTED.swap(true, Ordering::AcqRel) {
        return screen_capture_permission();
    }
    let accepted = objc2_core_graphics::CGRequestScreenCaptureAccess();
    let preflight = objc2_core_graphics::CGPreflightScreenCaptureAccess();
    coosenpai_core::ports::ScreenCapturePermission::after_request(accepted, preflight)
}

pub async fn read_activity() -> Result<ActivitySnapshot> {
    Ok(ActivitySnapshot {
        idle_ms: read_hid_idle_ms().await?,
        front_app: read_front_app_name().await?,
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DisplayGeometry {
    pub physical_width: f64,
    pub logical_width: f64,
}

pub async fn read_display_geometry() -> Option<DisplayGeometry> {
    let output = run_system_command(
        "/usr/sbin/system_profiler",
        &["SPDisplaysDataType", "-json"],
        Duration::from_secs(5),
    )
    .await
    .ok()?;
    let value = serde_json::from_str::<Value>(&output).ok()?;
    find_display_geometry(&value)
}

pub fn comparison_top_pixels(
    geometry: Option<&DisplayGeometry>,
    image_width: u32,
    image_height: u32,
) -> u32 {
    let scale = geometry
        .filter(|geometry| geometry.logical_width > 0.0)
        .map(|geometry| f64::from(image_width) / geometry.logical_width)
        .or_else(|| {
            geometry
                .filter(|geometry| geometry.logical_width > 0.0)
                .map(|geometry| geometry.physical_width / geometry.logical_width)
        });
    let top = scale
        .filter(|value| value.is_finite() && *value > 0.0)
        .map_or_else(|| f64::from(image_height) * 0.015, |value| 28.0 * value);
    top.round().max(1.0) as u32
}

pub async fn is_on_battery() -> bool {
    run_system_command("/usr/bin/pmset", &["-g", "batt"], Duration::from_secs(2))
        .await
        .map(|output| output.contains("Battery Power"))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct ActivitySnapshot {
    pub idle_ms: f64,
    pub front_app: String,
}

#[derive(Debug, Clone)]
pub struct OcrBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct OcrResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<OcrBlock>,
}

pub async fn recognize_text(path: &Path, level: &str, timeout: Duration) -> Result<Vec<OcrBlock>> {
    let path = path.to_owned();
    let level = level.to_owned();
    let task = tokio::task::spawn_blocking(move || recognize_text_blocking(&path, &level));
    tokio::time::timeout(timeout, task)
        .await
        .context("Vision OCR が timeout しました")??
}

/// Vision の同期 API が timeout 後も実行を占有するため、運用時は helper subprocess を優先する。
pub async fn recognize_text_with_helper(
    executable: &Path,
    paths: &[PathBuf],
    level: &str,
    timeout: Duration,
    cancellation: CancellationToken,
) -> Result<Vec<OcrResult>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![
        "--level".to_owned(),
        level.to_owned(),
        "--languages".to_owned(),
        "ja,en".to_owned(),
    ];
    args.extend(paths.iter().map(|path| path.display().to_string()));
    let runner = TokioProcessRunner;
    let output = runner
        .run(
            ProcessRequest {
                executable: executable.to_owned(),
                args,
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout,
            },
            cancellation,
        )
        .await?;
    if output.status != Some(0) {
        anyhow::bail!("OCR helper が失敗しました")
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.len() != paths.len() {
        anyhow::bail!("OCR helper の画像ごとの結果数が不正です")
    }
    let mut results = Vec::with_capacity(lines.len());
    for (index, line) in lines.into_iter().enumerate() {
        let raw: HelperResult =
            serde_json::from_str(line).context("OCR helper の JSON が不正です")?;
        if raw.path != paths[index].display().to_string()
            || raw.width == 0
            || raw.height == 0
            || raw.blocks.len() > 10_000
        {
            anyhow::bail!("OCR helper の画像結果が不正です")
        }
        let blocks = raw
            .blocks
            .into_iter()
            .map(|block| {
                if !block.x.is_finite()
                    || !block.y.is_finite()
                    || !block.w.is_finite()
                    || !block.h.is_finite()
                    || !block.confidence.is_finite()
                    || !(0.0..=1.0).contains(&block.x)
                    || !(0.0..=1.0).contains(&block.y)
                    || !(0.0..=1.0).contains(&block.w)
                    || !(0.0..=1.0).contains(&block.h)
                    || !(0.0..=1.0).contains(&block.confidence)
                {
                    return Err(anyhow::anyhow!("OCR helper のブロック座標が不正です"));
                }
                Ok(OcrBlock {
                    text: block.text,
                    x: block.x,
                    y: block.y,
                    width: block.w,
                    height: block.h,
                    confidence: block.confidence,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        results.push(OcrResult {
            path: PathBuf::from(raw.path),
            width: raw.width,
            height: raw.height,
            blocks,
        });
    }
    Ok(results)
}

#[derive(Debug, Deserialize)]
struct HelperResult {
    path: String,
    width: u32,
    height: u32,
    blocks: Vec<HelperBlock>,
}

#[derive(Debug, Deserialize)]
struct HelperBlock {
    text: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    confidence: f32,
}

async fn run_system_command(file: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let runner = TokioProcessRunner;
    let output = runner
        .run(
            ProcessRequest {
                executable: PathBuf::from(file),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout,
            },
            CancellationToken::new(),
        )
        .await?;
    if output.status != Some(0) {
        anyhow::bail!("macOS system command が失敗しました")
    }
    String::from_utf8(output.stdout).context("macOS system command の出力が UTF-8 ではありません")
}

fn parse_hid_idle_ms(output: &str) -> Result<f64> {
    let marker = "HIDIdleTime";
    let start = output
        .find(marker)
        .context("HIDIdleTime が見つかりません")?;
    let digits = output[start..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let nanoseconds = digits.parse::<u64>().context("HIDIdleTime が不正です")? as f64;
    Ok(nanoseconds / 1_000_000.0)
}

fn parse_front_app_asn(output: &str) -> Result<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("[ NULL ]") {
        anyhow::bail!("前面アプリの ASN が見つかりません")
    }
    trimmed
        .split_whitespace()
        .find(|value| value.starts_with("ASN:"))
        .or_else(|| trimmed.split_whitespace().next())
        .map(str::to_owned)
        .context("前面アプリの ASN が見つかりません")
}

fn parse_quoted_field(output: &str, key: &str) -> Result<String> {
    let key_start = output
        .find(key)
        .ok_or_else(|| anyhow::anyhow!("{key} が見つかりません"))?;
    let equals = output[key_start..]
        .find('=')
        .map(|offset| key_start + offset)
        .ok_or_else(|| anyhow::anyhow!("{key} の値が見つかりません"))?;
    let start = output[equals + 1..]
        .find('"')
        .map(|offset| equals + 1 + offset + 1)
        .ok_or_else(|| anyhow::anyhow!("{key} の値が引用符で囲まれていません"))?;
    let mut value = String::new();
    let mut escaped = false;
    for character in output[start..].chars() {
        if escaped {
            value.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            if value.is_empty() {
                anyhow::bail!("{key} の値が空です")
            }
            return Ok(value);
        } else {
            value.push(character);
        }
    }
    anyhow::bail!("{key} の値が閉じていません")
}

fn find_display_geometry(value: &Value) -> Option<DisplayGeometry> {
    match value {
        Value::Object(object) => {
            let physical_width = read_resolution_width(
                object,
                &["spdisplays_resolution", "resolution", "Resolution"],
            );
            let logical_width = read_resolution_width(
                object,
                &["spdisplays_ui_resolution", "ui_resolution", "UI Resolution"],
            );
            if let (Some(physical_width), Some(logical_width)) = (physical_width, logical_width) {
                if physical_width > 0.0 && logical_width > 0.0 {
                    return Some(DisplayGeometry {
                        physical_width,
                        logical_width,
                    });
                }
            }
            object.values().find_map(find_display_geometry)
        }
        Value::Array(values) => values.iter().find_map(find_display_geometry),
        _ => None,
    }
}

fn read_resolution_width(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .find_map(parse_resolution_width)
}

fn parse_resolution_width(value: &str) -> Option<f64> {
    let mut start = None;
    let mut end = None;
    for (index, character) in value.char_indices() {
        if character.is_ascii_digit() {
            if start.is_none() {
                start = Some(index);
            }
            end = Some(index + character.len_utf8());
        } else if let (Some(start_index), Some(end_index)) = (start, end) {
            let rest = value[end_index..].trim_start();
            if rest.starts_with('x') || rest.starts_with('×') {
                return value[start_index..end_index].parse().ok();
            }
            start = None;
            end = None;
        }
    }
    if let (Some(start), Some(end)) = (start, end) {
        let rest = value[end..].trim_start();
        if rest.starts_with('x') || rest.starts_with('×') {
            return value[start..end].parse().ok();
        }
    }
    None
}

fn recognize_text_blocking(path: &Path, level: &str) -> Result<Vec<OcrBlock>> {
    let normalized_image = normalized_cg_image(path)?;
    let options: objc2::rc::Retained<NSDictionary<VNImageOption, objc2::runtime::AnyObject>> =
        NSDictionary::new();
    // SAFETY: `normalized_image` and `options` are retained objects created above and match the
    // initializer's ownership contract.
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &normalized_image,
            &options,
        )
    };
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(if level == "fast" {
        VNRequestTextRecognitionLevel::Fast
    } else {
        VNRequestTextRecognitionLevel::Accurate
    });
    let languages = [NSString::from_str("ja-JP"), NSString::from_str("en-US")];
    let language_array = NSArray::from_retained_slice(&languages);
    request.setRecognitionLanguages(&language_array);
    request.setUsesLanguageCorrection(true);
    let request_for_array: objc2::rc::Retained<VNRequest> =
        request.retain().into_super().into_super();
    let requests = NSArray::from_retained_slice(&[request_for_array]);
    handler
        .performRequests_error(&requests)
        .map_err(|_| anyhow::anyhow!("Vision OCR request が失敗しました"))?;
    let mut blocks = Vec::new();
    if let Some(results) = request.results() {
        for observation in (*results).iter() {
            let candidates = observation.topCandidates(1);
            let Some(candidate) = candidates.firstObject() else {
                continue;
            };
            // SAFETY: `observation` came from this request's Vision result collection and the
            // bounding box accessor is valid for every VNRecognizedTextObservation.
            let bounds = unsafe { observation.boundingBox() };
            blocks.push(OcrBlock {
                text: candidate.string().to_string(),
                x: bounds.origin.x,
                y: 1.0 - bounds.origin.y - bounds.size.height,
                width: bounds.size.width,
                height: bounds.size.height,
                confidence: candidate.confidence(),
            });
        }
    }
    Ok(blocks)
}

fn normalized_cg_image(path: &Path) -> Result<CFRetained<CGImage>> {
    let source = NSImage::initWithContentsOfFile(
        NSImage::alloc(),
        &NSString::from_str(&path.to_string_lossy()),
    )
    .context("Vision OCR の画像を読み込めません")?;
    // SAFETY: a null proposed rectangle asks NSImage for its native CGImage representation. The
    // other optional arguments are absent, so no borrowed pointers outlive this call.
    let image =
        unsafe { source.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
            .context("Vision OCR の CGImage を作成できません")?;
    let width = CGImage::width(Some(&image));
    let height = CGImage::height(Some(&image));
    let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .context("Vision OCR の sRGB 色空間を作成できません")?;
    // SAFETY: a null data pointer asks CoreGraphics to allocate the backing store. Width,
    // height, row size, color space and RGBA bitmap layout are mutually consistent.
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            width.saturating_mul(4),
            Some(&color_space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .context("Vision OCR の bitmap context を作成できません")?;
    CGContext::draw_image(
        Some(&context),
        CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)),
        Some(&image),
    );
    CGBitmapContextCreateImage(Some(&context)).context("Vision OCR の正規化画像を作成できません")
}

