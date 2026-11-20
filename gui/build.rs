//! 把 assets/icon.ico 嵌入 exe 资源（Explorer / 任务栏 / 窗口标题栏图标）。
//! 图标由 assets/generate_icon.py 程序化生成（重生成方法见该脚本头注释）。

fn main() {
    // 仅 Windows 需要；非 Windows 构建跳过（winresource 自身也是 no-op 安全的，
    // 但显式判断避免无关平台引入依赖行为）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("embedding icon resource");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
