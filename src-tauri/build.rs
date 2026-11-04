fn main() {
    // Native Windows resources must rebuild when artwork changes at the same path.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
