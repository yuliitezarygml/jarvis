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
    } else if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .files(["native/audio_win.c", "native/admin_win.c"])
            .compile("jarvis_audio");
        println!("cargo:rustc-link-lib=winmm");
        cc::Build::new()
            .cpp(true)
            // sherpa-onnx ships its Windows C++ dependencies with the static MSVC CRT.
            .static_crt(true)
            .file("native/desktop_win.cpp")
            .compile("jarvis_desktop");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=uuid");
        println!("cargo:rerun-if-changed=native/desktop_win.cpp");
        println!("cargo:rustc-link-lib=wintrust");
        println!("cargo:rustc-link-lib=advapi32");
        println!("cargo:rustc-link-lib=shell32");
        println!("cargo:rerun-if-changed=native/admin_win.c");
        println!("cargo:rerun-if-changed=native/audio_win.c");
    }
    tauri_build::build()
}
