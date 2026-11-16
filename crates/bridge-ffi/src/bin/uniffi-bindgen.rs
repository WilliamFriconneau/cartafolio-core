//! Generates the Swift bindings: `cargo run -p memcp-bridge-ffi --bin uniffi-bindgen -- generate --library <lib> --language swift --out-dir <dir>`.

fn main() {
    uniffi::uniffi_bindgen_main()
}
