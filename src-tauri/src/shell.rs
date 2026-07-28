//! 窗口外壳：关窗即隐藏、菜单栏托盘、以及「什么才算真退出」。
//!
//! ## 为什么关窗不该是退出
//!
//! GenDesk 不是一个用完就关的工具，它是一台**长期在跑的机器**：即梦轮询器、交接目录
//! watcher、生图收件 watcher、常驻补单队列，全部只在进程活着时才存在。而 Tauri 的默认
//! 行为是「窗口全关 = 进程退出」，于是一次 ⌘W 就能把整条流水线停掉 —— 而人按下它时
//! 想的往往只是「这个窗口先收起来」。非 VIP 队列一排就是十几个小时，这中间界面本来
//! 就不需要开着；恰恰是那段时间最不能让进程死掉。
//!
//! 所以：**关窗一律只隐藏**（`CloseRequested` → `prevent_close` + `hide`），
//! 真退出只有两个入口，都在本模块里，都要经过 [`request_quit`]。
//!
//! ## 为什么必须换掉 macOS 那个预设的「退出」
//!
//! 实测（muda 0.19 `platform_impl/macos`）：`PredefinedMenuItem::quit` 在 macOS 上挂的是
//! `sel!(terminate:)` —— 它直接让 NSApplication 结束进程，**根本不经过 tao 的事件循环**，
//! 于是 `RunEvent::ExitRequested` 一次都不会发。也就是说，只要还用着那个预设项，
//! 「退出前确认一下有没有任务在跑」这件事在 ⌘Q 这条路径上就无从挂载。
//! 故这里自建一份与 Tauri 默认菜单同形的菜单，只把那一项换成我们自己的。
//!
//! ## 为什么不去拦 `ExitRequested`
//!
//! 因为拦了会误伤更新器：`AppHandle::restart()`（「重启安装」）走的正是同一个事件。
//! 而窗口既然永不销毁，「窗口全关」那条自动退出路径本来就不会触发 —— 于是退出只可能
//! 来自我们自己调的 [`request_quit`]，确认已经在那里做过了，再拦一道只会拦错人。

use std::sync::atomic::{AtomicI64, Ordering};

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

/// 自定义「退出」菜单项的 id（应用菜单与托盘菜单共用一条处理路径）。
const QUIT_ID: &str = "gendesk-quit";
/// 托盘菜单里的「显示主窗口」。
const SHOW_ID: &str = "gendesk-show";

/// 最近一次算出来的待办数。托盘标题只在**变了**的时候才写。
///
/// 托盘更新要过一次主线程往返，而 `v2v://changed` 在批量提交时每条推一次 ——
/// 不去重的话，一次 20 条的提交就是 20 次没有任何视觉变化的主线程调度。
/// 初值 -1（而不是 0）：真实的 0 也必须写一次，否则启动就没有待办时托盘是空的。
static LAST_BADGE: AtomicI64 = AtomicI64::new(-1);

/// 把主窗口叫回来。隐藏之后 `set_focus` 单独用是没有效果的，必须先 `show`。
///
/// 这三步一个都不能少：`show` 管隐藏、`unminimize` 管最小化，`set_focus` 管
/// 「窗口在，但被别的应用压在后面」。人点托盘/Dock 图标时并不知道自己处在哪一种。
pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 关窗只隐藏，不退出。挂在主窗口上。
///
/// 原来这里是「有未完成任务就弹确认框，否则直接放行关闭」。现在关闭不再丢任何东西，
/// 那个确认框也就没有存在的理由了 —— 它整个挪进了 [`request_quit`]，即真退出那条路径。
pub fn install_close_to_hide(window: &tauri::WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });
}

/// 真退出：先问「还有任务在跑吗」，确认了才结束进程。
///
/// 沿用 E26 那句文案与那条判据（`q`/`run`/`retry` 三个在制状态）—— 它要防的事没有变，
/// 变的只是触发它的时机：从「关窗」挪到了「退出」。空闲时不打扰，直接退。
///
/// 确认框是**非阻塞**的（`show` 带回调），因为这里跑在主线程上，阻塞式对话框会把
/// 事件循环连同它自己一起卡住。
pub fn request_quit(app: &AppHandle) {
    let state = app.state::<crate::state::AppState>();
    let pending: i64 = tauri::async_runtime::block_on(async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks WHERE status IN ('q','run','retry')",
        )
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
    });
    if pending == 0 {
        app.exit(0);
        return;
    }
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    let handle = app.clone();
    app.dialog()
        .message(format!(
            "仍有 {pending} 个任务未完成，退出将中断当前生成。\
             下次启动可继续未完成的任务。\
             （只想收起界面的话，关掉窗口就行 —— 那不会中断任何东西。）确定退出吗？"
        ))
        .title("确认退出 GenDesk")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "退出".into(),
            "取消".into(),
        ))
        .show(move |confirmed| {
            if confirmed {
                handle.exit(0);
            }
        });
}

/// 菜单/托盘菜单的点击分发。两处共用，保证「退出」只有一种含义。
fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        QUIT_ID => request_quit(app),
        SHOW_ID => show_main_window(app),
        _ => {}
    }
}

/// 菜单栏托盘图标 —— 后台常驻时它是这个进程唯一的存在证明。
///
/// 左键点图标 = 显示主窗口。这是 macOS 上最省事的那条路（不必先展开菜单再点一项），
/// 而菜单里仍然留着同一项，因为 Windows 上左键行为不统一。
pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, SHOW_ID, "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "退出 GenDesk", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep, &quit])?;

    TrayIconBuilder::with_id("gendesk-tray")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::AssetNotFound("默认窗口图标".into()))?,
        )
        .tooltip("GenDesk")
        .menu(&menu)
        // 左键点图标时不要顺带弹菜单，否则「点一下把窗口叫回来」会被菜单挡住。
        .show_menu_on_left_click(false)
        // **这里故意不挂 `on_menu_event`**。它听起来像「只管托盘这份菜单」，实际不是：
        // Tauri 把它 push 进的是与 `App::on_menu_event` 同一个全局监听器列表
        // （`TrayIcon::register`，其文档也明写「任何来源的菜单事件都会调它」）。
        // 两处都挂 = 每次点菜单跑两遍分发，而「退出」跑两遍就是弹两个确认框。
        // 托盘菜单的点击由 [`install_menu_handler`] 那一份统一收。
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 应用菜单：与 Tauri 默认菜单同形，只把「退出」换成我们自己的那一项。
///
/// 逐项抄一遍是有代价的（Tauri 以后给默认菜单加东西，这里不会自动跟上），但没有别的
/// 办法：`Menu::default` 里那个 `PredefinedMenuItem::quit` 在 macOS 上是 `terminate:`，
/// 它绕过整个事件循环，没有任何钩子挂得上去。而「⌘Q 之前问一句」正是这次要的东西。
///
/// **⌘W 不在这里换**：它对应的 `close_window` 预设项发的是 `performClose:`，
/// 那条路走的是窗口的 `CloseRequested` —— 也就是 [`install_close_to_hide`]，正合我们的意。
pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let pkg = app.package_info().clone();
    let quit = MenuItem::with_id(app, QUIT_ID, "退出 GenDesk", true, Some("CmdOrCtrl+Q"))?;

    let about_metadata = tauri::menu::AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        ..Default::default()
    };

    let window_menu = Submenu::with_items(
        app,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            // 「关闭窗口」现在的语义是收起来，标题就该这么说 —— 叫「关闭」会让人以为
            // 这是退出的同义词，而那正是这次要拆开的两件事。
            &PredefinedMenuItem::close_window(app, Some("收起窗口（后台继续跑）"))?,
        ],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata.clone()))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?,
            &Submenu::with_items(
                app,
                "文件",
                true,
                &[
                    &PredefinedMenuItem::close_window(app, Some("收起窗口（后台继续跑）"))?,
                    #[cfg(not(target_os = "macos"))]
                    &quit,
                ],
            )?,
            &edit_menu,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "显示",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            #[cfg(not(target_os = "macos"))]
            &Submenu::with_items(
                app,
                "帮助",
                true,
                &[&PredefinedMenuItem::about(app, None, Some(about_metadata))?],
            )?,
        ],
    )
}

/// 应用菜单的点击分发（托盘那份在 [`install_tray`] 里单独挂）。
pub fn install_menu_handler(app: &AppHandle) {
    app.on_menu_event(|app, event| on_menu_event(app, &event));
}

/// 把「有几件事等着人」写到托盘上。
///
/// 后台常驻带来的新问题正是这个：进程在跑，但界面收起来了，于是「有 3 条待验收」
/// 这件事没有任何地方说得出口 —— 而人收起窗口恰恰是因为它当时没事可做。
/// 托盘标题是这条信息唯一还看得见的落点。
///
/// 数字口径直接用 `StageCounts::actionable`（Rust 侧单点），不在这里另算一份：
/// 「什么算待办」这条规则会随流水线演进，抄一份出来就会与侧栏徽章悄悄分叉。
pub fn set_badge(app: &AppHandle, actionable: i64) {
    if LAST_BADGE.swap(actionable, Ordering::Relaxed) == actionable {
        return;
    }
    let Some(tray) = app.tray_by_id("gendesk-tray") else {
        return;
    };
    let tip = if actionable > 0 {
        format!("GenDesk · {actionable} 件事等你")
    } else {
        "GenDesk · 无待办".to_string()
    };
    let _ = tray.set_tooltip(Some(&tip));
    // 标题只有 macOS 显示得出来（Windows 托盘没有文字位）。0 时不占地方。
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(if actionable > 0 {
        Some(actionable.to_string())
    } else {
        None
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // 徽章去重：`v2v://changed` 在批量提交时每条推一次，而托盘更新要过一次主线程。
    // 没有这道闸，一次 20 条的提交就是 20 次没有任何视觉变化的主线程调度。
    #[test]
    fn badge_writes_are_deduplicated_but_the_first_zero_still_writes() {
        LAST_BADGE.store(-1, Ordering::Relaxed);
        let changed = |n: i64| LAST_BADGE.swap(n, Ordering::Relaxed) != n;
        assert!(
            changed(0),
            "初值必须是 -1，否则「启动就没待办」这一次会被吞掉"
        );
        assert!(!changed(0));
        assert!(changed(3));
        assert!(!changed(3));
        assert!(changed(0));
    }
}
