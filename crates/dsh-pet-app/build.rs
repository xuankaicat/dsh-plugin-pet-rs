//! Windows：把 assets/tray-icon.png 生成的 tray-icon.ico 嵌入 exe 作为程序图标。

fn main() {
    // 图标变化时让 cargo 重新执行 build.rs
    println!("cargo:rerun-if-changed=../../assets/tray-icon.ico");

    // 仅 Windows 需要；其他平台跳过资源编译
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winres::WindowsResource::new();
        // 路径相对于 crate 根（crates/dsh-pet-app）→ 工作区根 assets/
        res.set_icon("../../assets/tray-icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=图标资源编译失败（不影响构建）: {e}");
        }
    }
}
