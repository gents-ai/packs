//! Gives the WebAssembly build an 8 MiB stack, the size the Afterburner spike measured
//! (the default 1 MiB is too small for deeply nested scripts).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-zstack-size=8388608");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
