use crate::web::WebSearchCredential;
use crate::{
    EditTool, ListTool, ReadTool, SearchTool, ShellTool, WebFetchTool, WebSearchTool, WriteTool,
};
use bt_core::{BelltowerConfig, SessionToolMode, ToolSpec, traits::ToolExecutor};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct BuiltInToolRegistry {
    tools: BTreeMap<String, Arc<dyn ToolExecutor>>,
}

impl BuiltInToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_mode(SessionToolMode::Extended)
    }

    #[must_use]
    pub fn new_with_mode(mode: SessionToolMode) -> Self {
        let config = BelltowerConfig::from_embedded().expect("embedded Belltower config is valid");
        Self::new_with_config(mode, &config)
    }

    #[must_use]
    pub fn new_with_config(mode: SessionToolMode, config: &BelltowerConfig) -> Self {
        Self::new_with_config_and_web_credentials(mode, config, Vec::new())
    }

    #[must_use]
    pub fn new_with_config_and_web_credentials(
        mode: SessionToolMode,
        config: &BelltowerConfig,
        web_credentials: Vec<WebSearchCredential>,
    ) -> Self {
        let mut registry = Self::default();
        registry.register(ReadTool);
        registry.register(WriteTool);
        registry.register(EditTool);
        registry.register(ListTool);
        registry.register(SearchTool);
        registry.register(ShellTool);
        if mode.is_extended() {
            registry.register(WebSearchTool::new_with_credentials(
                config.web.clone(),
                web_credentials,
            ));
            registry.register(WebFetchTool);
        }
        registry
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: ToolExecutor + 'static,
    {
        self.register_arc(Arc::new(tool) as Arc<dyn ToolExecutor>);
    }

    pub fn register_arc(&mut self, tool: Arc<dyn ToolExecutor>) {
        self.tools.insert(tool.spec().name.clone(), tool);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.tools.get(name).cloned()
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        let has_catalogue = self.tools.contains_key("catalogue");
        self.descriptors()
            .into_iter()
            .filter(|spec| !has_catalogue || !spec.metadata.should_defer)
            .map(sanitize_model_facing_spec)
            .collect()
    }

    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

fn sanitize_model_facing_spec(mut spec: ToolSpec) -> ToolSpec {
    let Some(schema) = spec.parameters_schema.as_object_mut() else {
        return spec;
    };

    if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|entry| entry.as_str() != Some("call_id"));
    }

    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove("call_id");
    }

    spec
}

#[cfg(test)]
mod tests {
    use super::BuiltInToolRegistry;
    use bt_core::{
        ApprovalRequirement, ToolContext, ToolDisplayGroup, ToolExecutionMode, ToolExecutor,
        ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
    };
    use serde_json::{Value, json};

    #[test]
    fn model_facing_specs_hide_internal_call_id_argument() {
        let registry = BuiltInToolRegistry::new();
        let shell = registry
            .specs()
            .into_iter()
            .find(|spec| spec.name == "shell")
            .expect("shell spec");

        let required = shell.parameters_schema["required"]
            .as_array()
            .expect("required array");
        assert!(
            !required
                .iter()
                .any(|entry| entry.as_str() == Some("call_id"))
        );
        assert!(
            shell.parameters_schema["properties"]
                .get("call_id")
                .is_none()
        );
    }

    #[derive(Clone)]
    struct DeferredTool;

    impl ToolExecutor for DeferredTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "external_lookup".to_owned(),
                description: "Deferred external tool".to_owned(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["call_id"],
                    "properties": {
                        "call_id": {"type": "string"}
                    }
                }),
                metadata: ToolMetadata {
                    risk_class: ToolRiskClass::High,
                    is_read_only: false,
                    is_concurrency_safe: false,
                    interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
                    execution_mode: ToolExecutionMode::Immediate,
                    should_defer: true,
                    catalogue_tags: vec!["external".to_owned()],
                    display_group: ToolDisplayGroup::External,
                },
            }
        }

        fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
            ApprovalRequirement::Always
        }

        fn execute(
            &self,
            _arguments: Value,
            _context: ToolContext,
        ) -> bt_core::traits::ToolFuture<'_> {
            Box::pin(async move {
                Ok(ToolResultEnvelope {
                    call_id: bt_core::ToolCallId::new("call-1"),
                    tool_name: "external_lookup".to_owned(),
                    is_error: false,
                    output: json!({}),
                    duration_ms: None,
                })
            })
        }
    }

    #[test]
    fn deferred_tools_are_hidden_when_catalogue_is_present() {
        let mut registry = BuiltInToolRegistry::default();
        registry.register(DeferredTool);
        registry.register(crate::CatalogueTool::new(registry.descriptors()));

        let visible = registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert!(visible.contains(&"catalogue".to_owned()));
        assert!(!visible.contains(&"external_lookup".to_owned()));
    }

    #[test]
    fn standard_mode_hides_web_tools_but_extended_mode_keeps_them() {
        let standard = BuiltInToolRegistry::new_with_mode(bt_core::SessionToolMode::Standard);
        let extended = BuiltInToolRegistry::new_with_mode(bt_core::SessionToolMode::Extended);

        assert!(standard.get("web_search").is_none());
        assert!(standard.get("web_fetch").is_none());
        assert!(extended.get("web_search").is_some());
        assert!(extended.get("web_fetch").is_some());
    }

    #[test]
    fn initial_release_mode_inventories_are_explicit() {
        let standard = BuiltInToolRegistry::new_with_mode(bt_core::SessionToolMode::Standard)
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        let extended = BuiltInToolRegistry::new_with_mode(bt_core::SessionToolMode::Extended)
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert_eq!(
            standard,
            vec![
                "edit".to_owned(),
                "list".to_owned(),
                "read".to_owned(),
                "search".to_owned(),
                "shell".to_owned(),
                "write".to_owned(),
            ]
        );
        assert_eq!(
            extended,
            vec![
                "edit".to_owned(),
                "list".to_owned(),
                "read".to_owned(),
                "search".to_owned(),
                "shell".to_owned(),
                "web_fetch".to_owned(),
                "web_search".to_owned(),
                "write".to_owned(),
            ]
        );
    }
}
