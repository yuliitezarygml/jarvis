// Раскладка на сервере: /opt/jarvis-stt-rs/bin/<бинарь> и /opt/jarvis-stt-rs/lib/libsherpa-onnx-c-api.so,
// поэтому бинарям нужен rpath `$ORIGIN/../lib` (sherpa-onnx-sys добавляет только `$ORIGIN` и путь сборки).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let sherpa = std::env::var_os("CARGO_FEATURE_SHERPA").is_some();
    let unix = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux");
    if sherpa && unix {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../lib");
    }
}
