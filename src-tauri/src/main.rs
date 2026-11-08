// 生产构建下隐藏 Windows 控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("validate-intake") {
        let result = if args.len() == 2 {
            gendesk_lib::validate_intake_cli()
        } else {
            Err(gendesk_lib::AppError::InvalidInput(
                "validate-intake 仅从 stdin 接收 JSON".into(),
            ))
        };
        match result {
            Ok(value) => println!("{value}"),
            Err(error) => {
                println!(
                    "{}",
                    serde_json::json!({"status":"validation_error", "message":error.to_string()})
                );
                std::process::exit(2);
            }
        }
    } else if args.get(1).map(String::as_str) == Some("organize-library") {
        if let Err(error) = gendesk_lib::organize_library_cli(&args[2..]) {
            eprintln!("{error}");
            std::process::exit(1);
        }
    } else {
        gendesk_lib::run();
    }
}
