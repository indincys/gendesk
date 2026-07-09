//! GenDesk 应用库入口。
//!
//! 业务真相只在 Rust（技术文档 §1 铁律）；前端仅经 `src/lib/ipc/` 出入。
//! IPC 契约由 tauri-specta 在此集中登记，自动导出到 `src/lib/ipc/bindings.ts`。

mod commands;
mod db;
mod engine;
mod error;
mod files;
mod ids;
mod importer;
mod logging;
mod provider;
mod secrets;
mod state;

use std::sync::Arc;

use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

pub use error::{AppError, AppResult};

/// 构造 tauri-specta Builder —— 命令/事件的单一登记点。
///
/// `run()` 与 `export_bindings` 测试共用它，保证「运行时挂载的契约」与
/// 「导出给前端的绑定」永远同源（消除契约漂移，执行计划 §1.2）。
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::misc::log_frontend_error,
            // settings 域
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::pick_output_dir,
            commands::settings::open_logs_dir,
            commands::settings::open_path_in_folder,
            // api_keys 域
            commands::api_keys::list_api_keys,
            commands::api_keys::add_api_key,
            commands::api_keys::update_api_key,
            commands::api_keys::set_api_key_enabled,
            commands::api_keys::delete_api_key,
            // refs 域
            commands::refs::import_ref_images,
            commands::refs::list_ref_images,
            commands::refs::set_ref_image_group,
            // prompts 域
            commands::prompts::parse_prompt_txt,
            commands::prompts::commit_prompt_import,
            // batches / tasks 域（引擎）
            commands::batches::create_batch,
            commands::batches::list_batches,
            commands::batches::pause_queue,
            commands::batches::resume_queue,
            commands::tasks::list_tasks,
            commands::tasks::get_task,
            commands::tasks::retry_task,
            commands::tasks::retry_failed_tasks,
            commands::tasks::retry_interrupted_tasks,
            commands::tasks::count_interrupted,
        ])
        .events(collect_events![
            engine::events::TaskStatusChanged,
            engine::events::TaskProgress,
            engine::events::BatchSummary,
            engine::events::KeyHealth,
        ])
}

/// TS 导出配置：i64 → number（编号/计数均在 JS 安全整数范围内）。
#[cfg(any(debug_assertions, test))]
fn ts_config() -> specta_typescript::Typescript {
    use specta_typescript::{BigIntExportBehavior, Typescript};
    Typescript::default().bigint(BigIntExportBehavior::Number)
}

/// 应用主入口，由 `main.rs`（桌面）与移动端入口共同调用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // 开发构建时把最新绑定写出到前端；生产构建不含此步。
    #[cfg(debug_assertions)]
    {
        if let Err(err) = builder.export(ts_config(), "../src/lib/ipc/bindings.ts") {
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

            // 数据目录 + 日志 + DB 池 + 密钥存储 + 引擎，装配为 AppState。
            let data_dir = app.path().app_data_dir()?;
            let dirs = Arc::new(files::DataDirs::new(&data_dir));
            dirs.init()?;

            let guard = logging::init(&dirs.logs());
            app.manage(guard);

            let pool = tauri::async_runtime::block_on(db::connect(&dirs.db()))?;
            let secrets: Arc<dyn secrets::SecretStore> = Arc::new(secrets::KeyringStore);

            // 读设置 → 启动引擎（中断恢复 + 调度循环）。
            let default_out = dirs.outputs().to_string_lossy().to_string();
            let settings = tauri::async_runtime::block_on(commands::settings::load_settings(
                &pool,
                &default_out,
            ))?;
            let strategy =
                engine::strategy::Strategy::from_str_or_default(&settings.schedule_strategy);
            let factory: Arc<dyn engine::ProviderFactory> = Arc::new(engine::OpenAiFactory::new(
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(180),
            ));
            let sink: engine::events::SharedSink =
                Arc::new(engine::events::TauriSink::new(app.handle().clone()));
            let engine = tauri::async_runtime::block_on(engine::Engine::start(
                pool.clone(),
                dirs.clone(),
                factory,
                sink,
                strategy,
                settings.retry_count.max(0) as u32,
                settings.paused,
                secrets.clone(),
            ))?;

            app.manage(state::AppState::new(pool, secrets, dirs, Arc::new(engine)));

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
        // 重新导出并写盘；CI 随后 `git diff --exit-code` 校验 bindings.ts 已同步。
        specta_builder()
            .export(ts_config(), "../src/lib/ipc/bindings.ts")
            .expect("导出 TypeScript 绑定失败");
    }
}
