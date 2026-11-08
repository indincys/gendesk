//! 构建脚本：macOS 上先把实况图助手 `live-photo/gendesk-live.swift` 编成 sidecar，
//! 再交给 tauri-build（它会校验 `tauri.macos.conf.json` 里登记的 externalBin 存在）。

use std::error::Error;
use std::path::Path;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var("CARGO_CFG_TARGET_OS")? == "macos" {
        build_live_photo_helper()?;
    }
    tauri_build::build();
    Ok(())
}

fn build_live_photo_helper() -> Result<(), Box<dyn Error>> {
    let source = Path::new("live-photo/gendesk-live.swift");
    println!("cargo:rerun-if-changed={}", source.display());
    let target = std::env::var("TARGET")?;
    let output = Path::new("binaries").join(format!("gendesk-live-{target}"));
    // tauri-build 的任何改动都会让本脚本重跑；源码没变就不再花几秒重编。
    let fresh = match (output.metadata(), source.metadata()) {
        (Ok(out), Ok(src)) => out.modified()? >= src.modified()?,
        _ => false,
    };
    if fresh {
        return Ok(());
    }
    std::fs::create_dir_all("binaries")?;
    let arch = target.split('-').next().unwrap_or("aarch64");
    let arch = if arch == "aarch64" { "arm64" } else { arch };
    let status = Command::new("swiftc")
        .args(["-O", "-swift-version", "5", "-target"])
        .arg(format!("{arch}-apple-macos12.0"))
        .arg(source)
        .arg("-o")
        .arg(&output)
        .status()?;
    if !status.success() {
        return Err(format!("swiftc 编译实况图助手失败：{status}").into());
    }
    Ok(())
}
