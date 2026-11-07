#![cfg_attr(all(target_os = "windows", feature = "desktop"), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMA-BUF renderer crashes on Wayland with NVIDIA
    // ("Error 71 (Protocol error) dispatching to Wayland display").
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|arg| arg == "--headless") {
        sprite_atlas_packer::run();
        return;
    }

    if let Err(error) = sprite_atlas_packer::run_cli(args) {
        eprintln!(
            "{}",
            serde_json::json!({
                "status": "error",
                "error": error.to_string()
            })
        );
        std::process::exit(1);
    }
}
