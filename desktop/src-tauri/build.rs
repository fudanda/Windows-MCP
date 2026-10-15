fn main() {
    println!("cargo:rerun-if-changed=../dist");
    println!("cargo:rerun-if-changed=../../scripts/windows-mcp-transfer.py");
    tauri_build::build()
}
