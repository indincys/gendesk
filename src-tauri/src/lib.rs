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
            commands::misc::app_version,
            // settings 域
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::pick_output_dir,
            commands::settings::pick_txt_file,
            commands::settings::pick_image_files,
            commands::settings::open_logs_dir,
            commands::settings::open_path_in_folder,
            // api_keys 域
            commands::api_keys::list_api_keys,
            commands::api_keys::add_api_key,
            commands::api_keys::update_api_key,
            commands::api_keys::set_api_key_enabled,
            commands::api_keys::recover_api_key,
            commands::api_keys::delete_api_key,
            commands::api_keys::test_api_key,
            commands::api_keys::test_api_key_saved,
            // refs 域
            commands::refs::import_ref_images,
            commands::refs::list_ref_images,
            commands::refs::set_ref_image_group,
            commands::refs::get_ref_image,
            commands::refs::replace_ref_image_file,
            commands::refs::trash_ref_image,
            // prompts 域
            commands::prompts::list_prompt_groups,
            commands::prompts::create_prompt_group,
            commands::prompts::list_prompts,
            commands::prompts::search_prompts,
            commands::prompts::get_prompt,
            commands::prompts::update_prompt_text,
            commands::prompts::toggle_prompt_favorite,
            commands::prompts::trash_prompt,
            commands::prompts::parse_prompt_txt,
            commands::prompts::commit_prompt_import,
            // batches / tasks 域（引擎）
            commands::batches::create_batch,
            commands::batches::estimate_task_seconds,
            commands::batches::cancel_batch_pending,
            commands::batches::list_batches,
            commands::batches::get_batch_config,
            commands::batches::pause_queue,
            commands::batches::resume_queue,
            commands::tasks::list_tasks,
            commands::tasks::get_task,
            commands::tasks::retry_task,
            commands::tasks::retry_failed_tasks,
            commands::tasks::delete_task,
            commands::tasks::delete_failed_tasks,
            commands::tasks::retry_interrupted_tasks,
            commands::tasks::count_interrupted,
            // review 域
            commands::review::list_pending_review,
            commands::review::accept_tasks,
            commands::review::reject_tasks,
            // works 域
            commands::works::list_works,
            commands::works::get_work,
            commands::works::toggle_work_favorite,
            commands::works::trash_work,
            // trash 域
            commands::trash::list_trash,
            commands::trash::purge_trash_items,
            commands::trash::purge_all_trash,
            commands::trash::count_trash,
            // updater 域
            commands::updater::check_update_now,
            commands::updater::install_update,
            // backup 域（E19 数据备份与数据目录可见性）
            commands::backup::data_dir_info,
            commands::backup::open_data_dir,
            commands::backup::export_backup,
        ])
        .events(collect_events![
            engine::events::TaskStatusChanged,
            engine::events::TaskProgress,
            engine::events::BatchSummary,
            engine::events::KeyHealth,
            commands::updater::UpdateStateChanged,
            commands::backup::BackupProgress,
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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(commands::updater::PendingUpdate::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // 事件挂载（当前无事件；M1/M2 增加后由 builder 统一登记）。
            builder.mount_events(app);

            // 初始化失败（数据库 schema 版本高于当前应用/降级、目录不可写等）不再
            // 让 Tauri 在 setup 返回 Err 时内部 panic → SIGABRT 崩溃弹窗；改为记录后
            // 干净退出（退出码 1），原因落入日志文件供排查。
            if let Err(err) = setup_app(app) {
                tracing::error!(error = %err, "应用初始化失败，退出");
                eprintln!("[gendesk] 应用初始化失败: {err}");
                std::process::exit(1);
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

/// setup 钩子中所有可失败的初始化步骤。抽出为独立函数，使调用方能把失败
/// 转成干净退出，而非让 Tauri 在 setup 返回 `Err` 时内部 panic（abort → 崩溃弹窗）。
///
/// 典型失败：数据库 schema 版本高于当前应用（旧包对新库 / 降级），sqlx 迁移会
/// 报 `VersionMissing`；此前该错误经 `?` 上抛 → Tauri panic → SIGABRT。
fn setup_app(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
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
    let settings =
        tauri::async_runtime::block_on(commands::settings::load_settings(&pool, &default_out))?;
    let strategy = engine::strategy::Strategy::from_str_or_default(&settings.schedule_strategy);
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
    engine.set_global_fail_threshold(settings.global_fail_threshold.max(0) as u32);

    app.manage(state::AppState::new(pool, secrets, dirs, Arc::new(engine)));

    // Windows：无系统装饰，改由前端自绘窗控（macOS 保留 Overlay 交通灯）。
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(windows)]
        {
            let _ = window.set_decorations(false);
        }

        // E26：关闭窗口拦截——有未完成任务（排队/生成中/重试中）时先确认，
        // 避免误退中断跑批；空闲时直接关闭无打扰。确认退出走既有中断恢复路径
        // （下次启动 run/retry → Interrupted，可一键继续）。
        let handle = window.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = handle.app_handle();
                let state = app.state::<state::AppState>();
                let pending: i64 = tauri::async_runtime::block_on(async {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM tasks WHERE status IN ('q','run','retry')",
                    )
                    .fetch_one(&state.db)
                    .await
                    .unwrap_or(0)
                });
                if pending == 0 {
                    return; // 空闲：不拦截
                }
                api.prevent_close();
                use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                let win = handle.clone();
                handle
                    .dialog()
                    .message(format!(
                        "仍有 {pending} 个任务未完成，退出将中断当前生成。\
                         下次启动可继续未完成的任务。确定退出吗？"
                    ))
                    .title("确认退出")
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "退出".into(),
                        "取消".into(),
                    ))
                    .show(move |confirmed| {
                        if confirmed {
                            let _ = win.destroy();
                        }
                    });
            }
        });
    }

    Ok(())
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
