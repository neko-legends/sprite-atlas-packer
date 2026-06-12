#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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
