//! Один `ToolRegistry` из инструментов всех модулей. `ToolRegistry::register` паникует на дубликате имени,
//! поэтому имена проверяются заранее: при совпадении остаётся инструмент модуля, стоящего раньше в списке
//! (порядок приоритета: sys → pc → web → kb → memory), остальные пропускаются с предупреждением.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_core::{Tool, ToolRegistry};

/// Итог сборки реестра.
pub struct BuiltRegistry {
    pub registry: ToolRegistry,
    /// Имя → модуль, чей инструмент остался.
    pub owners: BTreeMap<String, &'static str>,
    /// (имя, модуль, который пропущен, модуль-владелец).
    pub duplicates: Vec<(String, &'static str, &'static str)>,
}

pub fn build_registry(groups: Vec<(&'static str, Vec<Arc<dyn Tool>>)>) -> BuiltRegistry {
    let mut registry = ToolRegistry::new();
    let mut owners: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut duplicates = Vec::new();
    for (module, tools) in groups {
        for tool in tools {
            let name = tool.spec().name;
            if let Some(owner) = owners.get(&name) {
                eprintln!("[jarvis] инструмент «{name}» есть и в {owner}, и в {module} — оставлен из {owner}");
                duplicates.push((name, module, *owner));
                continue;
            }
            owners.insert(name, module);
            registry.register(tool);
        }
    }
    BuiltRegistry { registry, owners, duplicates }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use jarvis_core::{RiskTier, ToolCtx, ToolError, ToolOutput, ToolSpec};

    struct T(&'static str);
    #[async_trait]
    impl Tool for T {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.0.into(), description: "x".into(), parameters: serde_json::json!({"type":"object"}), tier: RiskTier::T0 }
        }
        async fn call(&self, _a: serde_json::Value, _c: &ToolCtx) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text("ok"))
        }
    }

    #[test]
    fn first_module_wins_without_panic() {
        let b = build_registry(vec![
            ("sys", vec![Arc::new(T("open")) as Arc<dyn Tool>, Arc::new(T("a"))]),
            ("web", vec![Arc::new(T("open")) as Arc<dyn Tool>, Arc::new(T("b"))]),
        ]);
        assert_eq!(b.registry.len(), 3);
        assert_eq!(b.owners["open"], "sys");
        assert_eq!(b.duplicates, vec![("open".to_string(), "web", "sys")]);
    }
}
