//! GenDesk 应用库入口。
//!
//! 业务真相只在 Rust（技术文档 §1 铁律）；前端仅经 `src/lib/ipc/` 出入。
//! IPC 契约由 tauri-specta 在此集中登记，自动导出到 `src/lib/ipc/bindings.ts`。

mod commands;
mod error;
mod logging;

use tauri::Manager;
use tauri_specta::{collect_commands, Builder};

pub use error::{AppError, AppResult};

/// 构造 tauri-specta Builder —— 命令/事件的单一登记点。
///
/// `run()` 与 `export_bindings` 测试共用它，保证「运行时挂载的契约」与
/// 「导出给前端的绑定」永远同源（消除契约漂移，执行计划 §1.2）。
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![commands::misc::log_frontend_error])
}

/// 应用主入口，由 `main.rs`（桌面）与移动端入口共同调用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // 开发构建时把最新绑定写出到前端；生产构建不含此步。
    #[cfg(debug_assertions)]
    {
        use specta_typescript::Typescript;
        if let Err(err) = builder.export(Typescript::default(), "../src/lib/ipc/bindings.ts") {
            eprintln!("[specta] 导出 bindings.ts 失败: {err}");
        }
    }

    tauri::Builder::default()
        // single-instance 必须最先注册（保护任务队列与号池，禁双开）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // 事件挂载（当前无事件；M1/M2 增加后由 builder 统一登记）。
            builder.mount_events(app);

            // 日志初始化到 app_data/logs，guard 托管到应用状态保持存活。
            if let Ok(data_dir) = app.path().app_data_dir() {
                let guard = logging::init(&data_dir.join("logs"));
                app.manage(guard);
            }

            // Windows：无系统装饰，改由前端自绘窗控（macOS 保留 Overlay 交通灯）。
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(windows)]
                {
                    let _ = window.set_decorations(false);
                }
                let _ = window;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // 启动失败无法进入日志系统，退回 stderr 并非零退出。
            eprintln!("[gendesk] 应用启动失败: {e}");
            std::process::exit(1);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试中允许 unwrap/expect：断言失败即测试失败，是期望行为。
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    #[test]
    fn export_bindings() {
        use specta_typescript::Typescript;
        // 重新导出并写盘；CI 随后 `git diff --exit-code` 校验 bindings.ts 已同步。
        specta_builder()
            .export(Typescript::default(), "../src/lib/ipc/bindings.ts")
            .expect("导出 TypeScript 绑定失败");
    }
}
