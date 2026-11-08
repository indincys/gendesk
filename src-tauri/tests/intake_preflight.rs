#![allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions and fixture setup.

use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

fn validate(input: &serde_json::Value, home: &std::path::Path) -> (bool, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_gendesk"))
        .arg("validate-intake")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let report = serde_json::from_slice(&output.stdout);
    assert!(
        report.is_ok(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.unwrap();
    (output.status.success(), report)
}

#[test]
fn real_binary_preflight_decodes_images_normalizes_params_and_has_no_app_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("product.png");
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 3, image::Rgb([1, 2, 3])))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(&source, bytes.get_ref()).unwrap();
    let mut input = serde_json::json!({"version":1,"order":{"version":1,
        "organization":{"requirement":"接口预检"},"defaults":{"ratio":"3:4","format":"jpg"},
        "items":[{"name":"场景样本","ref":"refs/product.png"}]},
        "prompts":{"场景样本":"Image 1 是产品，保留其结构。"},"references":{"refs/product.png":source}});
    let (ok, report) = validate(&input, &home);
    assert!(ok, "{report}");
    assert_eq!(report["validation"], "full");
    assert_eq!(report["references"][0]["width"], 2);
    assert_eq!(report["references"][0]["height"], 3);
    assert_eq!(
        report["references"][0]["sha256"].as_str().unwrap().len(),
        64
    );
    let params: serde_json::Value =
        serde_json::from_str(report["items"][0]["paramsJson"].as_str().unwrap()).unwrap();
    assert_eq!(params["size"], "1536x2048");
    assert_eq!(params["quality"], "high");
    assert_eq!(params["outputFormat"], "jpeg");
    input["order"]["defaults"]["size"] = "1080x1920".into();
    let (ok, report) = validate(&input, &home);
    assert!(!ok);
    assert!(report["message"].as_str().unwrap().contains("16 的倍数"));
    input["order"]["defaults"]
        .as_object_mut()
        .unwrap()
        .remove("size");
    input["order"]["purpose"] = "unknown-purpose".into();
    assert!(!validate(&input, &home).0);
    input["order"].as_object_mut().unwrap().remove("purpose");
    std::fs::write(&source, b"not an image").unwrap();
    let (ok, report) = validate(&input, &home);
    assert!(!ok);
    assert!(report["message"].as_str().unwrap().contains("解码失败"));
    assert!(
        !home.exists(),
        "Headless preflight must not create app data, inbox, database or receipts"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
