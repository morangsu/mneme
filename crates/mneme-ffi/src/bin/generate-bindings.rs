//! 生成 Kotlin 绑定：
//! cargo run -p mneme-ffi --bin generate-bindings -- generate \
//!   --library target/debug/mneme_ffi.dll --language kotlin --out-dir <out>
fn main() {
    uniffi::uniffi_bindgen_main()
}
