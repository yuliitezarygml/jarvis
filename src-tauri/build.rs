fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("native/audio.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("jarvis_audio");
        for framework in ["Foundation", "AVFoundation", "Metal"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        println!("cargo:rerun-if-changed=native/audio.m");
    }
    tauri_build::build()
}
