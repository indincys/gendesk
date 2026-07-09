// 生产构建下隐藏 Windows 控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    gendesk_lib::run();
}
