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
mod intake;
mod logging;
mod provider;
mod publish;
mod purpose;
mod secrets;
mod state;
mod v2v;

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
            commands::settings::open_output_dir,
            commands::settings::open_path_in_folder,
            commands::settings::diagnostics_info,
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
            commands::refs::scan_ref_imports,
            commands::refs::list_ref_images,
            commands::refs::list_ref_groups,
            commands::refs::create_ref_group,
            commands::refs::rename_ref_group,
            commands::refs::delete_ref_group,
            commands::refs::set_ref_image_group,
            commands::refs::set_ref_images_group,
            commands::refs::set_ref_image_archived,
            commands::refs::get_ref_image,
            commands::refs::replace_ref_image_file,
            commands::refs::trash_ref_image,
            commands::refs::trash_ref_images,
            // prompts 域
            commands::prompts::list_prompt_groups,
            commands::prompts::create_prompt_group,
            commands::prompts::rename_prompt_group,
            commands::prompts::set_prompt_group_archived,
            commands::prompts::set_prompt_group_purposes,
            commands::prompts::list_purposes,
            commands::prompts::backfill_group_purposes,
            commands::prompts::delete_prompt_group,
            commands::prompts::merge_prompt_groups,
            commands::prompts::move_prompts_to_group,
            commands::prompts::set_prompts_favorite,
            commands::prompts::trash_prompts,
            commands::prompts::list_prompts,
            commands::prompts::search_prompts,
            commands::prompts::get_prompt,
            commands::prompts::update_prompt_text,
            commands::prompts::toggle_prompt_favorite,
            commands::prompts::trash_prompt,
            commands::prompts::parse_prompt_txt,
            commands::prompts::repreview_import,
            commands::prompts::commit_prompt_import,
            commands::prompts::save_prompt_template,
            // batches / tasks 域（引擎）
            commands::batches::create_batch,
            commands::batches::estimate_task_seconds,
            commands::batches::cancel_batch_pending,
            commands::batches::list_batches,
            commands::batches::get_batch_config,
            commands::batches::rename_batch,
            commands::batches::pause_queue,
            commands::batches::resume_queue,
            commands::batches::open_batch_output_dir,
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
            commands::works::file_exists,
            commands::works::reexport_work,
            commands::works::set_works_favorite,
            commands::works::trash_works,
            commands::works::export_works,
            commands::works::export_works_v2v,
            // intake 域（Claude Code / Codex 投单 → 自动建批）
            commands::intake::get_intake_settings,
            commands::intake::update_intake_settings,
            commands::intake::intake_pending_dir,
            commands::intake::list_intake_jobs,
            commands::intake::scan_intake_now,
            commands::intake::retry_intake_job,
            commands::intake::confirm_intake_job,
            commands::intake::open_intake_dir,
            commands::intake::pick_intake_root,
            // v2v 域（图生视频流水线）
            commands::v2v::get_v2v_settings,
            commands::v2v::update_v2v_settings,
            commands::v2v::pick_handoff_root,
            commands::v2v::pick_dreamina_bin,
            commands::v2v::resolve_v2v_bin,
            commands::v2v::v2v_models,
            commands::v2v::v2v_credit,
            commands::v2v::v2v_credit_stats,
            commands::v2v::v2v_queue_stats,
            commands::v2v::v2v_sessions,
            commands::v2v::v2v_effective_params,
            commands::v2v::v2v_activity,
            commands::v2v::clear_v2v_activity,
            commands::v2v::list_v2v_clips,
            commands::v2v::v2v_counts,
            commands::v2v::enqueue_works_v2v,
            commands::v2v::materialize_v2v_handoff,
            commands::v2v::ingest_v2v_rewrites,
            commands::v2v::open_handoff_dir,
            commands::v2v::update_v2v_clip,
            commands::v2v::set_v2v_clip_params,
            commands::v2v::preview_v2v_commands,
            commands::v2v::submit_v2v_clips,
            commands::v2v::poll_v2v_now,
            commands::v2v::review_v2v_clips,
            commands::v2v::requeue_v2v_clips,
            commands::v2v::remove_v2v_clips,
            // stats 域（E25）
            commands::stats::list_group_stats,
            commands::stats::prompt_stats,
            commands::stats::production_overview,
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
            // ── 发布与资产管理模块（P1 起）──────────────────────────
            // publish_settings 域
            commands::publish_settings::get_publish_settings,
            commands::publish_settings::update_publish_settings,
            commands::publish_settings::pick_publish_root,
            commands::publish_settings::use_local_as_exec_root,
            commands::publish_settings::publish_platforms,
            // skus 域
            commands::publish_skus::list_skus,
            commands::publish_skus::create_sku,
            commands::publish_skus::update_sku,
            commands::publish_skus::set_sku_status,
            commands::publish_skus::get_sku_detail,
            commands::publish_skus::get_publish_badges,
            commands::publish_skus::import_sku_mappings,
            commands::publish_skus::pick_mapping_file,
            commands::publish_skus::save_sku_mapping_template,
            // texts 域
            commands::publish_texts::list_text_items,
            commands::publish_texts::add_text_item,
            commands::publish_texts::update_text_item,
            commands::publish_texts::set_text_item_enabled,
            // assets 域
            commands::publish_assets::list_asset_packs,
            commands::publish_assets::import_media_files,
            commands::publish_assets::pack_from_works,
            commands::publish_assets::pack_from_clip,
            commands::publish_assets::retire_pack,
            commands::publish_assets::restore_pack,
            commands::publish_assets::delete_pack,
            commands::publish_assets::update_pack,
            commands::publish_assets::activate_pack,
            // inbox 域
            commands::publish_inbox::list_inbox_items,
            commands::publish_inbox::claim_inbox_item,
            commands::publish_inbox::discard_inbox_item,
            commands::publish_inbox::retry_inbox_item,
            commands::publish_inbox::rescan_inbox,
            // accounts 域
            commands::publish_accounts::list_accounts,
            commands::publish_accounts::create_account,
            commands::publish_accounts::update_account,
            commands::publish_accounts::set_account_status,
            // planning 域（P2）
            commands::publish_planning::generate_sheet,
            commands::publish_planning::list_sheets,
            commands::publish_planning::get_sheet,
            commands::publish_planning::confirm_sheet,
            commands::publish_planning::unlock_sheet,
            commands::publish_planning::update_task_row,
            commands::publish_planning::cancel_task_row,
            commands::publish_planning::delete_task_row,
            commands::publish_planning::add_task_row,
            commands::publish_planning::reroll_set,
            commands::publish_planning::list_schedulable_skus,
            commands::publish_accounts::delete_account,
            commands::publish_texts::delete_text_item,
            commands::publish_skus::restock_prompt,
            commands::publish_assets::import_text_file,
            commands::publish_assets::pack_history,
            commands::publish_insights::preview_schedule,
            commands::publish_insights::calendar_month,
            commands::publish_insights::daily_brief,
            commands::publish_planning::preflight_export,
            commands::publish_planning::export_package,
            commands::publish_planning::open_package_dir,
            // reconcile 域（P3）
            commands::publish_reconcile::import_receipts,
            commands::publish_reconcile::resolve_suspect,
            commands::publish_reconcile::get_dashboard,
            commands::publish_reconcile::get_report,
        ])
        .events(collect_events![
            engine::events::TaskStatusChanged,
            engine::events::TaskProgress,
            engine::events::BatchSummary,
            engine::events::KeyHealth,
            commands::updater::UpdateStateChanged,
            commands::backup::BackupProgress,
            commands::refs::RefImportProgress,
            // 发布模块事件（P1 起）
            publish::events::PublishBadgesEvent,
            publish::events::InboxIngestEvent,
            publish::events::SheetChangedEvent,
            publish::events::ExportProgressEvent,
            // 生图工单收件事件
            commands::intake::IntakeChanged,
            // 视频流水线事件
            v2v::events::V2vChanged,
            v2v::events::V2vProgress,
            v2v::events::V2vActivity,
            v2v::events::V2vTick,
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

    // 密钥存本地加密文件（见 secrets.rs 模块头安全水位）。旧版把 Key 放系统钥匙串，
    // 自签名下每次更新都弹授权 → 启动时做一次性迁移（幂等，失败只 warn 不阻断启动，
    // 下次启动重试）。必须发生在 Engine::start 之前，保证引擎首读即走文件、零弹窗。
    let store = Arc::new(secrets::FileStore::new(&data_dir)?);
    if let Err(err) =
        tauri::async_runtime::block_on(secrets::migrate_from_keyring(&pool, store.as_ref()))
    {
        tracing::warn!(error = %err, "钥匙串密钥迁移未完成，下次启动重试");
    }
    let secrets: Arc<dyn secrets::SecretStore> = store;

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

    // E22/E40（决策 D3）：启动时到期自动清理归档批次与废纸篓（0 天 = 关闭）。
    tauri::async_runtime::block_on(commands::trash::run_startup_cleanup(
        &pool,
        settings.batch_retention_days,
        settings.trash_retention_days,
    ));

    let dirs_for_v2v = dirs.clone();
    let dirs_for_intake = dirs.clone();
    // 引擎句柄要分给工单收件（建批后唤醒调度器），故先 Arc 起来再交给 AppState。
    let engine = Arc::new(engine);
    let engine_for_intake = engine.clone();
    // 视频流水线执行日志：轮询器 / 提交 / 交接监听共用同一份环形缓冲，
    // 命令层从 AppState 读它。在 manage 之前建好，好让后台任务拿到同一个句柄。
    let v2v_log = v2v::activity::Activity::new(app.handle().clone());
    app.manage(state::AppState::new(
        pool.clone(),
        secrets,
        dirs,
        engine,
        v2v_log.clone(),
    ));

    // 发布模块：启动收件箱监听 + 启动补跑收录（若已配置本机根目录）。
    let publish_state = publish::PublishState::new(pool.clone(), app.handle().clone());
    if let Ok(pset) = tauri::async_runtime::block_on(commands::publish_settings::load(&pool)) {
        if !pset.root_local.is_empty() {
            let root = std::path::PathBuf::from(&pset.root_local);
            if let Err(err) = publish_state.restart(root.clone()) {
                tracing::warn!(error = %err, "启动收件箱监听失败");
            }
            // 启动补跑：全量扫描收件箱（异步，不阻塞启动）。
            let pool_bg = pool.clone();
            let app_bg = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = publish::inbox::ingest::rescan(&pool_bg, &root).await {
                    tracing::warn!(error = %err, "启动补跑收录失败");
                }
                publish::inbox::watcher::emit_badges(&pool_bg, &app_bg).await;
            });
            // 应用内定时：启动补跑生成今日/明日草稿 + 每 5 分钟一轮。
            publish::ticker::spawn(pool.clone(), app.handle().clone());
        }
    }
    app.manage(publish_state);

    // 生图工单收件：监听收件目录（skill 投单）+ 启动补跑一次。
    //
    // **启动补跑是这条链路的关键**：应用没开的时候投的单，磁盘上一直等着；
    // 这一刻把它们收进来，「GenDesk 得开着才能投单」这个限制就不存在了。
    if let Ok(iset) = tauri::async_runtime::block_on(commands::intake::load_settings(&pool)) {
        if iset.enabled {
            let root = iset.root_path();
            let ctx = intake::ingest::Ctx {
                pool: pool.clone(),
                dirs: dirs_for_intake,
                engine: engine_for_intake,
                threshold: iset.task_threshold,
            };
            match intake::watcher::start(ctx.clone(), root.clone(), app.handle().clone()) {
                Ok(w) => {
                    app.manage(w);
                }
                Err(err) => tracing::warn!(error = %err, "启动生图收件目录监听失败"),
            }
            let app_bg = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::intake::scan_and_emit(&ctx, &root, &app_bg).await;
            });
        }
    }

    // 视频流水线：监听交接目录（skill 写回改写结果）+ 后台轮询已提交条目。
    //
    // 启动即物化一次工单：上次退出后新验收的条目、或用户手动改过交接根，
    // 都要在这一刻把磁盘上的工单对齐成库里的真相，否则 skill 会对着旧内容干活。
    if let Ok(vset) = tauri::async_runtime::block_on(commands::v2v::load_settings(&pool)) {
        let root = vset.root();
        match v2v::watcher::start(
            pool.clone(),
            root.clone(),
            app.handle().clone(),
            v2v_log.clone(),
        ) {
            Ok(w) => {
                app.manage(w);
            }
            Err(err) => tracing::warn!(error = %err, "启动交接目录监听失败"),
        }
        let pool_bg = pool.clone();
        let app_bg = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            // 先收录一次（应用没开时 skill 也可能写回了），再按最新队列物化。
            if let Err(err) = v2v::handoff::ingest(&pool_bg, &root).await {
                tracing::warn!(error = %err, "启动补跑收录改写结果失败");
            }
            commands::v2v::refresh_handoff(&pool_bg, &app_bg).await;
        });
        v2v::runner::spawn(
            pool.clone(),
            dirs_for_v2v,
            app.handle().clone(),
            v2v_log.clone(),
        );
    }

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
