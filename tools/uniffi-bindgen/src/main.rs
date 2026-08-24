//! Bindings generator for `prohori-ffi`.
//!
//! uniffi's own CLI, wrapped so the version that reads the library is exactly the
//! version that generated its scaffolding. Both come from the workspace's single pinned
//! `uniffi` dependency, which is the point — mismatched halves of a uniffi pair produce
//! a Kotlin file that compiles and then misreads the FFI buffer at runtime.
//!
//! Usage (see `core-ffi/README.md`):
//!
//! ```text
//! cargo run -p prohori-uniffi-bindgen -- generate \
//!     --library target/debug/libprohori_ffi.so \
//!     --language kotlin --out-dir app/src/main/java
//! ```

fn main() {
    uniffi::uniffi_bindgen_main();
}
