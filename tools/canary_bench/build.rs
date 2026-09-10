fn main() {
    // The app obtains this link through CTranslate2; this standalone driver
    // must link the OpenMP runtime required by its transcribe.cpp build.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=gomp");
    }
}
