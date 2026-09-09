use anyhow::{Context, Result};
use coosenpai_core::ports::{OwnWindowBounds, WindowBounds};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};

/// メインスレッド上で自プロセスのウィンドウだけを取得する。
pub fn current_process_window_bounds(changes: &crate::OwnWindowChanges) -> Result<OwnWindowBounds> {
    current_process_window_bounds_with(changes, read_window_frames)
}

fn current_process_window_bounds_with(
    changes: &crate::OwnWindowChanges,
    read_frames: impl FnOnce() -> Result<(f64, Vec<WindowFrame>)>,
) -> Result<OwnWindowBounds> {
    let (primary_height, windows) = read_frames()?;
    let mut bounds = collect_window_bounds(primary_height, windows)?;
    bounds.revision = changes.revision();
    Ok(bounds)
}

fn read_window_frames() -> Result<(f64, Vec<WindowFrame>)> {
    let main = MainThreadMarker::new().context("自ウィンドウの取得にはメインスレッドが必要です")?;
    let application = NSApplication::sharedApplication(main);
    let primary = CGDisplayBounds(CGMainDisplayID());
    let windows = application
        .windows()
        .iter()
        .map(|window| WindowFrame {
            visible: window.isVisible(),
            frame: window.frame(),
        })
        .collect();
    Ok((primary.size.height, windows))
}

struct WindowFrame {
    visible: bool,
    frame: CGRect,
}

fn collect_window_bounds(
    primary_height: f64,
    windows: impl IntoIterator<Item = WindowFrame>,
) -> Result<OwnWindowBounds> {
    anyhow::ensure!(
        primary_height.is_finite() && primary_height > 0.0,
        "主ディスプレイの寸法が不正です"
    );
    let mut bounds = Vec::new();
    for window in windows {
        if !window.visible {
            continue;
        }
        let frame = window.frame;
        anyhow::ensure!(
            frame.origin.x.is_finite()
                && frame.origin.y.is_finite()
                && frame.size.width.is_finite()
                && frame.size.height.is_finite()
                && frame.size.width > 0.0
                && frame.size.height > 0.0,
            "自ウィンドウの寸法が不正です"
        );
        // AppKit は主画面の左下、Quartz は左上が原点。倍率は画像処理時に適用する。
        bounds.push(WindowBounds {
            x: frame.origin.x,
            y: primary_height - frame.origin.y - frame.size.height,
            width: frame.size.width,
            height: frame.size.height,
        });
    }
    Ok(OwnWindowBounds {
        revision: 0,
        captured_at: chrono::Utc::now(),
        bounds,
    })
}

