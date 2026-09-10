fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    println!("cargo:rustc-link-search=native={root}/deps/usr/lib/x86_64-linux-gnu");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=gomp");
    }
}
