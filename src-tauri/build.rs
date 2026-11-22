//! Манифест Windows: release — `requireAdministrator` (Джарвис всегда от администратора, один UAC при старте),
//! debug — `asInvoker` (разработка и `cargo test` без UAC). Зависимость Common Controls v6 сохраняется.

fn manifest(level: &str) -> String {
    format!(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{level}" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#
    )
}

fn main() {
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let level = if release { "requireAdministrator" } else { "asInvoker" };
    println!("cargo:rerun-if-env-changed=PROFILE");

    let target = std::env::var("TARGET").unwrap_or_default();
    let windows = if target.contains("windows-msvc") {
        // tauri-build вшил бы манифест ресурсом только в бинарник, а тестовый exe без манифеста не получает
        // Common Controls v6 и падает с STATUS_ENTRYPOINT_NOT_FOUND. Поэтому манифест встраивает линкер MSVC —
        // во все исполняемые файлы пакета (приложение и `cargo test`), а из ресурса tauri-build он убран.
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let path = out.join("jarvis.manifest");
        std::fs::write(&path, manifest(level)).expect("запись манифеста");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", path.display());
        tauri_build::WindowsAttributes::new_without_app_manifest()
    } else {
        tauri_build::WindowsAttributes::new().app_manifest(manifest(level))
    };
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri-build");
}
