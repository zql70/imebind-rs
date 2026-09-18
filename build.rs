// 嵌入 exe 图标（资源段）。
//
// cargo 本身不支持给 exe 加图标资源，需要构建脚本调用资源编译器（rc.exe）把 .rc 编译成 .res
// 再交给链接器。这里用 embed-resource crate，它会自动在 Windows SDK 里找 rc.exe。
//
// 目的：让 exe 文件在资源管理器里显示的图标和托盘图标一致。
// 注意托盘图标是运行时从 exe 同目录的 icon.ico 读的（见 tray.rs），
// 这里是编进 exe 资源段的另一份，两者都来自同一个 icon.ico。
fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    println!("cargo:rerun-if-changed=icon.rc");

    // manifest_optional()：非 Windows 目标、或环境里找不到资源编译器（比如只装了 GNU 工具链）
    // 都算成功，只有"能编译但编译失败"才返回 Err。这样别人 clone 下来不会因为缺 SDK 而构建失败，
    // 代价只是 exe 用默认图标。
    if embed_resource::compile("icon.rc", embed_resource::NONE)
        .manifest_optional()
        .is_err()
    {
        println!("cargo:warning=嵌入 exe 图标失败，exe 将使用默认图标");
    }
}
