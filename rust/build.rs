fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // The worker can transcribe audio files without Media Foundation.
        for dll in ["mfplat.dll", "mfreadwrite.dll"] {
            println!("cargo:rustc-link-arg-bin=transcribe=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-bin=transcribe=delayimp.lib");
    }
}
