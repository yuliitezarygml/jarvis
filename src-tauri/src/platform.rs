use crate::tools::Telemetry;
use anyhow::Result;
use serde_json::Value;

#[async_trait::async_trait]
pub trait PlatformBackend {
    async fn system_info(&self, telemetry: &Telemetry) -> Result<Value>;
}

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::WindowsBackend as NativeBackend;

#[cfg(not(windows))]
pub struct NativeBackend;
#[cfg(not(windows))]
#[async_trait::async_trait]
impl PlatformBackend for NativeBackend {
    async fn system_info(&self, telemetry: &Telemetry) -> Result<Value> {
        Ok(telemetry.sample())
    }
}
