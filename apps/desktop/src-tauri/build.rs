use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let helper_script = manifest_dir.join("../../../tools/ocr-helper/build.sh");
    let helper_source = manifest_dir.join("../../../tools/ocr-helper/Sources/main.swift");
    let speech_script = manifest_dir.join("../../../tools/speech-helper/build.sh");
    let speech_source = manifest_dir.join("../../../tools/speech-helper/Sources/main.swift");
    let speech_plist = manifest_dir.join("../../../tools/speech-helper/Info.plist");
    let hearing_script = manifest_dir.join("../../../tools/hearing-helper/build.sh");
    let hearing_sources = manifest_dir.join("../../../tools/hearing-helper/Sources");
    let hearing_source = manifest_dir.join("../../../tools/hearing-helper/Sources/main.swift");
    let hearing_stats_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_stats.swift");
    let hearing_scaling_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_scaling.swift");
    let hearing_buffer_copy_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_buffer_copy.swift");
    let hearing_conversion_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_conversion.swift");
    let hearing_recognition_state_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/recognition_state.swift");
    let hearing_segment_controller_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/segment_controller.swift");
    let hearing_voice_activity_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/voice_activity.swift");
    let hearing_wav_input_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/wav_input.swift");
    let hearing_appended_audio_dump_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/appended_audio_dump.swift");
    let hearing_tap_header =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_tap_installer.h");
    let hearing_tap_source =
        manifest_dir.join("../../../tools/hearing-helper/Sources/audio_tap_installer.m");
    let hearing_plist = manifest_dir.join("../../../tools/hearing-helper/Info.plist");
    let target = std::env::var("TARGET")?;
    let bridge_script = manifest_dir.join("../../../tools/provider-bridge/build.sh");
    let bridge_package = manifest_dir.join("../../../tools/provider-bridge/package.json");
    let bridge_source = manifest_dir.join("../../../tools/provider-bridge/src");
    println!("cargo:rerun-if-changed={}", helper_script.display());
    println!("cargo:rerun-if-changed={}", helper_source.display());
    println!("cargo:rerun-if-changed={}", speech_script.display());
    println!("cargo:rerun-if-changed={}", speech_source.display());
    println!("cargo:rerun-if-changed={}", speech_plist.display());
    println!("cargo:rerun-if-changed={}", hearing_script.display());
    println!("cargo:rerun-if-changed={}", hearing_sources.display());
    println!("cargo:rerun-if-changed={}", hearing_source.display());
    println!("cargo:rerun-if-changed={}", hearing_stats_source.display());
    println!(
        "cargo:rerun-if-changed={}",
        hearing_conversion_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_scaling_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_buffer_copy_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_recognition_state_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_segment_controller_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_voice_activity_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_wav_input_source.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        hearing_appended_audio_dump_source.display()
    );
    println!("cargo:rerun-if-changed={}", hearing_tap_header.display());
    println!("cargo:rerun-if-changed={}", hearing_tap_source.display());
    println!("cargo:rerun-if-changed={}", hearing_plist.display());
    println!("cargo:rerun-if-changed={}", bridge_package.display());
    println!("cargo:rerun-if-changed={}", bridge_source.display());
    let status = Command::new(&helper_script).arg(&target).status()?;
    if !status.success() {
        return Err(format!("OCR helper のビルドに失敗しました: {status}").into());
    }
    let speech_status = Command::new(&speech_script).arg(&target).status()?;
    if !speech_status.success() {
        return Err(format!("音声認識 helper のビルドに失敗しました: {speech_status}").into());
    }
    let hearing_status = Command::new(&hearing_script).arg(&target).status()?;
    if !hearing_status.success() {
        return Err(format!("聴覚観察 helper のビルドに失敗しました: {hearing_status}").into());
    }
    let bridge_status = Command::new(&bridge_script).status()?;
    if !bridge_status.success() {
        return Err(format!("provider bridge のビルドに失敗しました: {bridge_status}").into());
    }
    let bridge = manifest_dir.join("../../../tools/provider-bridge/dist/bridge.js");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .ok_or("Cargo profile ディレクトリを取得できません")?;
    std::fs::copy(&bridge, profile_dir.join("provider-bridge.js"))?;
    tauri_build::build();
    Ok(())
}
