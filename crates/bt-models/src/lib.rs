#![forbid(unsafe_code)]

use bt_core::{
    BelltowerConfig, BelltowerError, HardwareProfile, ModelBackendConfig, ModelBackendDescriptor,
    ModelBackendKind, ModelBackendStatus, ModelRecommendation, ModelRecommendationsReport, Result,
};
use reqwest::{Client, Url};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct LocalModelManager {
    backends: Vec<ModelBackendConfig>,
    http: Client,
}

impl LocalModelManager {
    pub fn from_config(config: &BelltowerConfig) -> Result<Self> {
        Ok(Self {
            backends: config.models.backends.clone(),
            http: Client::builder()
                .timeout(Duration::from_millis(config.models.request_timeout_ms))
                .build()
                .map_err(|error| BelltowerError::Protocol(error.to_string()))?,
        })
    }

    pub async fn backends(&self) -> Vec<ModelBackendDescriptor> {
        let mut descriptors = Vec::with_capacity(self.backends.len());
        for backend in &self.backends {
            descriptors.push(self.probe_backend(backend).await);
        }
        descriptors
    }

    pub fn recommendations(&self) -> Result<ModelRecommendationsReport> {
        let hardware = detect_hardware_profile();
        let catalog = BelltowerConfig::model_recommendations()?;
        let memory = hardware.total_memory_gb;
        let recommendations = catalog
            .tier
            .into_iter()
            .map(|tier| ModelRecommendation {
                label: tier.label,
                max_memory_gb: tier.max_memory_gb,
                models: tier.models,
                fits_hardware: memory.is_some_and(|available| available >= tier.max_memory_gb),
            })
            .collect();
        Ok(ModelRecommendationsReport {
            hardware,
            recommendations,
        })
    }

    async fn probe_backend(&self, backend: &ModelBackendConfig) -> ModelBackendDescriptor {
        if !backend.enabled {
            return ModelBackendDescriptor {
                kind: backend.kind.clone(),
                label: backend_label(&backend.kind),
                base_url: backend.base_url.clone(),
                status: ModelBackendStatus::Disabled,
                available_models: Vec::new(),
            };
        }

        let url = model_endpoint(&backend.base_url, &backend.kind);
        let response = self.http.get(url).send().await;
        match response {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.json::<Value>().await {
                    Ok(value) => ModelBackendDescriptor {
                        kind: backend.kind.clone(),
                        label: backend_label(&backend.kind),
                        base_url: backend.base_url.clone(),
                        status: ModelBackendStatus::Ready,
                        available_models: parse_model_names(&backend.kind, value),
                    },
                    Err(error) => ModelBackendDescriptor {
                        kind: backend.kind.clone(),
                        label: backend_label(&backend.kind),
                        base_url: backend.base_url.clone(),
                        status: ModelBackendStatus::Unreachable {
                            reason: error.to_string(),
                        },
                        available_models: Vec::new(),
                    },
                },
                Err(error) => ModelBackendDescriptor {
                    kind: backend.kind.clone(),
                    label: backend_label(&backend.kind),
                    base_url: backend.base_url.clone(),
                    status: ModelBackendStatus::Unreachable {
                        reason: error.to_string(),
                    },
                    available_models: Vec::new(),
                },
            },
            Err(error) => ModelBackendDescriptor {
                kind: backend.kind.clone(),
                label: backend_label(&backend.kind),
                base_url: backend.base_url.clone(),
                status: ModelBackendStatus::Unreachable {
                    reason: error.to_string(),
                },
                available_models: Vec::new(),
            },
        }
    }
}

fn backend_label(kind: &ModelBackendKind) -> String {
    match kind {
        ModelBackendKind::Ollama => "Ollama".to_owned(),
        ModelBackendKind::LmStudio => "LM Studio".to_owned(),
        ModelBackendKind::LlamaCpp => "llama.cpp".to_owned(),
    }
}

fn model_endpoint(base_url: &Url, kind: &ModelBackendKind) -> Url {
    let mut base = base_url.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(match kind {
        ModelBackendKind::Ollama => "api/tags",
        ModelBackendKind::LmStudio | ModelBackendKind::LlamaCpp => "v1/models",
    })
    .expect("model endpoint should join")
}

fn parse_model_names(kind: &ModelBackendKind, payload: Value) -> Vec<String> {
    match kind {
        ModelBackendKind::Ollama => payload
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("name").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect(),
        ModelBackendKind::LmStudio | ModelBackendKind::LlamaCpp => payload
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect(),
    }
}

fn detect_hardware_profile() -> HardwareProfile {
    HardwareProfile {
        total_memory_gb: detect_total_memory_gb(),
    }
}

fn detect_total_memory_gb() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let raw = String::from_utf8(output.stdout).ok()?;
        let bytes = raw.trim().parse::<u64>().ok()?;
        return Some(bytes.div_ceil(1 << 30));
    }

    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = raw.lines().find(|line| line.starts_with("MemTotal:"))?;
        let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        return Some((kb * 1024).div_ceil(1 << 30));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::{LocalModelManager, parse_model_names};
    use axum::Json;
    use axum::routing::get;
    use axum::{Router, serve};
    use bt_core::{BelltowerConfig, ModelBackendKind, ModelBackendStatus};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn detects_ollama_and_openai_style_backends() {
        async fn ollama_models() -> Json<Value> {
            Json(json!({
                "models": [
                    { "name": "qwen2.5-coder:7b" },
                    { "name": "phi4-mini" }
                ]
            }))
        }

        async fn openai_models() -> Json<Value> {
            Json(json!({
                "data": [
                    { "id": "qwen3-8b" },
                    { "id": "codestral-22b" }
                ]
            }))
        }

        let ollama_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ollama");
        let ollama_addr = ollama_listener.local_addr().expect("ollama addr");
        tokio::spawn(async move {
            serve(
                ollama_listener,
                Router::new().route("/api/tags", get(ollama_models)),
            )
            .await
            .expect("ollama server");
        });

        let lm_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind lm studio");
        let lm_addr = lm_listener.local_addr().expect("lm studio addr");
        tokio::spawn(async move {
            serve(
                lm_listener,
                Router::new().route("/v1/models", get(openai_models)),
            )
            .await
            .expect("lm studio server");
        });

        let mut config = BelltowerConfig::from_embedded().expect("config");
        config.models.backends[0].base_url =
            format!("http://{ollama_addr}").parse().expect("ollama url");
        config.models.backends[1].base_url =
            format!("http://{lm_addr}").parse().expect("lm studio url");
        config.models.backends[2].enabled = false;

        let manager = LocalModelManager::from_config(&config).expect("manager");
        let backends = manager.backends().await;
        assert_eq!(backends.len(), 3);
        assert!(matches!(backends[0].status, ModelBackendStatus::Ready));
        assert_eq!(
            backends[0].available_models,
            vec!["qwen2.5-coder:7b", "phi4-mini"]
        );
        assert!(matches!(backends[1].status, ModelBackendStatus::Ready));
        assert_eq!(
            backends[1].available_models,
            vec!["qwen3-8b", "codestral-22b"]
        );
        assert!(matches!(backends[2].status, ModelBackendStatus::Disabled));
    }

    #[test]
    fn parses_model_lists_for_each_backend_shape() {
        assert_eq!(
            parse_model_names(
                &ModelBackendKind::Ollama,
                json!({ "models": [{ "name": "foo" }] })
            ),
            vec!["foo"]
        );
        assert_eq!(
            parse_model_names(
                &ModelBackendKind::LmStudio,
                json!({ "data": [{ "id": "bar" }] })
            ),
            vec!["bar"]
        );
    }

    #[test]
    fn recommendations_mark_fitting_tiers() {
        let manager = LocalModelManager::from_config(
            &BelltowerConfig::from_embedded().expect("embedded config"),
        )
        .expect("manager");
        let report = manager.recommendations().expect("recommendations");
        assert!(!report.recommendations.is_empty());
    }
}
