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
    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest(level));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri-build");
}
