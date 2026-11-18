use crate::{audio, core::Core, tts};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Mutex;
use sysinfo::{Disks, System};
pub struct Telemetry {
    system: Mutex<System>,
    gpu: String,
}
impl Telemetry {
    pub fn new() -> Self {
        Self {
            system: Mutex::new(System::new_all()),
            gpu: audio::gpu(),
        }
    }
    pub fn sample(&self) -> Value {
        let mut s = self.system.lock().unwrap();
        s.refresh_cpu_usage();
        s.refresh_memory();
        json!({"cpu":s.global_cpu_usage(),"ram":s.used_memory(),"total":s.total_memory(),"gpu":self.gpu,"host":System::host_name(),"os":System::long_os_version(),"cores":s.cpus().len()})
    }
}
pub struct ToolRegistry;
const TOOLS: [(&str, &str); 7] = [
    ("get_time", "Текущее местное время"),
    ("system_info", "Операционная система и процессор"),
    ("cpu", "Загрузка процессора"),
    ("ram", "Использованная и доступная память"),
    ("disks", "Тома и свободное место"),
    ("server_status", "Доступность сервера через туннель"),
    ("tts_status", "Готовность голосового движка"),
];
impl ToolRegistry {
    pub fn schemas() -> Vec<Value> {
        TOOLS.iter().map(|(n,d)|json!({"type":"function","function":{"name":n,"description":d,"parameters":{"type":"object","properties":{},"additionalProperties":false}}})).collect()
    }
    pub async fn run(c: &Core, name: &str, args: &Value) -> Result<Value> {
        if !args.as_object().is_some_and(|o| o.is_empty()) {
            bail!("Инструмент принимает пустой объект");
        }
        Ok(match name{"get_time"=>json!({"time":chrono::Local::now().to_rfc3339()}),"system_info"=>c.telemetry.sample(),"cpu"=>json!({"percent":c.telemetry.sample()["cpu"]}),"ram"=>{let s=c.telemetry.sample();json!({"used":s["ram"],"total":s["total"]})},"disks"=>tokio::task::spawn_blocking(||Disks::new_with_refreshed_list().iter().map(|d|json!({"mount":d.mount_point(),"available":d.available_space(),"total":d.total_space()})).collect::<Vec<_>>()).await?.into(),"server_status"|"tts_status"=>tts::ready(&c.http).await?,_=>bail!("Действие не разрешено. Изменяющие инструменты требуют отдельного подтверждения и исполнителя.")})
    }
}
