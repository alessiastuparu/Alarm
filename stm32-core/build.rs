fn main() {
    println!("cargo:rustc-link-search={}", std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}