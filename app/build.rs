fn main() {
    println!("cargo:rerun-if-changed=../src/dashboard.html");
    tauri_build::build()
}
