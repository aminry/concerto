// Tauri 2 codegen hook. Generates the platform-specific glue (Info.plist
// keys on macOS, manifest on Windows, .desktop on Linux) and the
// `tauri::generate_context!()` payload that `main.rs` consumes.
fn main() {
    tauri_build::build();
}
