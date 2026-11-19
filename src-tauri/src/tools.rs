use crate::platform::{NativeBackend, PlatformBackend};
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
    ("server_status", "Доступность HTTPS API голосового сервера"),
    ("tts_status", "Готовность голосового движка"),
];
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Permission {
    ReadOnly,
    RequiresConfirmation,
    Unknown,
}
impl ToolRegistry {
    pub fn permission(name: &str) -> Permission {
        #[cfg(windows)]
        if matches!(
            name,
            "kb_status" | "kb_search" | "kb_lookup_command" | "command_help"
        ) || crate::platform::windows::DIAGNOSTICS
            .iter()
            .any(|d| d.name == name)
        {
            return Permission::ReadOnly;
        }
        if TOOLS.iter().any(|(registered, _)| *registered == name) {
            return Permission::ReadOnly;
        }
        match name {
            "manage_app" | "write_file" | "docker" | "ssh" | "automate" | "control_device" => {
                Permission::RequiresConfirmation
            }
            _ => Permission::Unknown,
        }
    }
    pub fn schemas() -> Vec<Value> {
        fn schema(name: &str, desc: &str, properties: Value, required: &[&str]) -> Value {
            json!({"type":"function","function":{"name":name,"description":desc,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}})
        }
        let schemas = TOOLS.iter().map(|(n, d)| schema(n, d, json!({}), &[]));
        #[cfg(windows)]
        let schemas=schemas.chain(crate::platform::windows::DIAGNOSTICS.iter().map(|d|schema(d.name,d.description,json!({}),&[])))
            .chain([
                schema("kb_status","Состав локальной справки Windows, версия и происхождение",json!({}),&[]),
                schema("kb_search","Search local Windows documentation in English. Cite repo and path.",json!({"query":{"type":"string","maxLength":160},"version":{"type":"string"},"limit":{"type":"integer","minimum":3,"maximum":5}}),&["query"]),
                schema("kb_lookup_command","Точная справка по имени команды",json!({"name":{"type":"string","maxLength":100}}),&["name"]),
                schema("command_help","Проверить наличие и справку командлета в Windows PowerShell",json!({"name":{"type":"string","maxLength":80}}),&["name"]),
            ]);
        schemas.collect()
    }
    pub async fn run(c: &Core, name: &str, args: &Value) -> Result<Value> {
        match Self::permission(name) {
            Permission::Unknown=>bail!("Неизвестный инструмент"),
            Permission::RequiresConfirmation=>bail!("Требуется отдельное подтверждение пользователя. Исполнитель этого действия ещё не подключён."),
            Permission::ReadOnly=>()
        }
        #[cfg(windows)]
        {
            if matches!(name, "kb_search" | "kb_lookup_command" | "kb_status") {
                return c.kb.run(name, args);
            }
            if name == "command_help"
                || crate::platform::windows::DIAGNOSTICS
                    .iter()
                    .any(|d| d.name == name)
            {
                return crate::platform::windows::diagnostic(name, args).await;
            }
        }
        if !args.as_object().is_some_and(|o| o.is_empty()) {
            bail!("Инструмент принимает пустой объект");
        }
        let result=match name {
            "get_time"=>json!({"time":chrono::Local::now().to_rfc3339()}),
            "system_info"=>NativeBackend.system_info(&c.telemetry).await?,
            "cpu"=>json!({"percent":c.telemetry.sample()["cpu"]}),
            "ram"=>{let sample=c.telemetry.sample();json!({"used":sample["ram"],"total":sample["total"]})},
            "disks"=>tokio::task::spawn_blocking(||{
                Disks::new_with_refreshed_list().iter().map(|disk| {
                    json!({"mount":disk.mount_point(),"available":disk.available_space(),"total":disk.total_space()})
                }).collect::<Vec<_>>()
            }).await?.into(),
            "server_status"|"tts_status"=>tts::ready(&c.http).await?,
            _=>bail!("Отсутствует обработчик инструмента")
        };
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_cannot_authorize_actions() {
        assert_eq!(ToolRegistry::permission("get_time"), Permission::ReadOnly);
        assert_eq!(
            ToolRegistry::permission("ssh"),
            Permission::RequiresConfirmation
        );
        assert_eq!(
            ToolRegistry::permission("arbitrary_shell"),
            Permission::Unknown
        );
    }
}
