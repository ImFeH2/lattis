#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().invoke_handler(tauri::generate_handler![]);

    #[cfg(any(target_os = "android", target_os = "ios"))]
    let builder = builder.plugin(tauri_plugin_tun_device::init());

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
