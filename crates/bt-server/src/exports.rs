//! Session export and OTLP push route handlers.
//!
//! These handlers translate protocol requests into runtime export bundles and
//! external OTLP collector pushes. Session storage/export semantics remain owned
//! by `bt-runtime`, `bt-session`, and `bt-otel`.

use super::{ApiError, ApiJson, AppState, require_session};
use axum::Json;
use axum::extract::{Path, State};
use bt_core::SessionId;
use bt_otel::{export_otlp_protobuf, export_session as render_session_export};
use bt_protocol::{
    ExportFormat, PushOtlpExportRequest, PushOtlpExportResponse, SessionExportResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use prost::Message as _;
use reqwest::header::{
    AUTHORIZATION as REQWEST_AUTHORIZATION, CONTENT_TYPE as REQWEST_CONTENT_TYPE,
    HeaderMap as ReqwestHeaderMap, HeaderName as ReqwestHeaderName,
    HeaderValue as ReqwestHeaderValue,
};
use url::Url;

pub(super) async fn session_export(
    State(state): State<AppState>,
    Path((session_id, format)): Path<(SessionId, String)>,
) -> Result<Json<SessionExportResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let format = ExportFormat::parse(&format)?;
    let bundle = state.runtime.export_legacy_bundle(session_id)?;
    let (content_type, content) = match format.clone() {
        ExportFormat::LegacyBundle => (
            format.content_type().to_owned(),
            serde_json::to_string_pretty(&bundle)
                .map_err(|error| bt_core::BelltowerError::Storage(error.to_string()))?,
        ),
        other => {
            let rendered =
                render_session_export(&bundle.session, &bundle.messages, &bundle.events, other)?;
            (rendered.content_type, rendered.content)
        }
    };
    Ok(Json(SessionExportResponse {
        session_id,
        format,
        content_type,
        content,
    }))
}

pub(super) async fn push_otlp_export(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ApiJson(request): ApiJson<PushOtlpExportRequest>,
) -> Result<Json<PushOtlpExportResponse>, ApiError> {
    let _ = require_session(&state, session_id)?;
    let bundle = state.runtime.export_legacy_bundle(session_id)?;
    let request_url = normalize_otlp_endpoint(request.endpoint.as_deref())?;
    let payload = export_otlp_protobuf(
        &bundle.session,
        &bundle.events,
        request.project_name.as_deref(),
    )?;
    let mut headers = resolved_otlp_headers(&request)?;
    headers.insert(
        REQWEST_CONTENT_TYPE,
        ReqwestHeaderValue::from_static("application/x-protobuf"),
    );
    let bytes_sent = payload.len();
    let response = reqwest::Client::new()
        .post(request_url.clone())
        .headers(headers)
        .body(payload)
        .send()
        .await
        .map_err(|error| {
            bt_core::BelltowerError::Protocol(format!("otlp export request failed: {error}"))
        })?;
    let status = response.status();
    let response_content_type = response
        .headers()
        .get(REQWEST_CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let response_body = response.bytes().await.map_err(|error| {
        bt_core::BelltowerError::Protocol(format!(
            "failed reading otlp collector response: {error}"
        ))
    })?;
    if !status.is_success() {
        let response_text = String::from_utf8_lossy(&response_body);
        let preview = response_text.trim();
        return Err(bt_core::BelltowerError::Protocol(format!(
            "otlp collector push failed with status {status}: {}",
            if preview.is_empty() {
                "<empty body>"
            } else {
                preview
            }
        ))
        .into());
    }
    let outcome =
        parse_otlp_success_response(response_content_type.as_deref(), response_body.as_ref());

    Ok(Json(PushOtlpExportResponse {
        session_id,
        request_url: request_url.to_string(),
        content_type: "application/x-protobuf".to_owned(),
        bytes_sent,
        status_code: status.as_u16(),
        rejected_spans: outcome.rejected_spans,
        warning: outcome.warning,
    }))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct OtlpPushOutcome {
    rejected_spans: Option<i64>,
    warning: Option<String>,
}

fn parse_otlp_success_response(content_type: Option<&str>, body: &[u8]) -> OtlpPushOutcome {
    if body.is_empty() {
        return OtlpPushOutcome::default();
    }

    let response = match decode_otlp_success_response(content_type, body) {
        Ok(response) => response,
        Err(error) => {
            return OtlpPushOutcome {
                rejected_spans: None,
                warning: Some(unreadable_otlp_success_warning(
                    content_type,
                    body,
                    error.as_str(),
                )),
            };
        }
    };

    let Some(partial_success) = response.partial_success else {
        return OtlpPushOutcome::default();
    };
    let warning = partial_success.error_message.trim();
    let rejected_spans =
        (partial_success.rejected_spans > 0).then_some(partial_success.rejected_spans);
    let warning = (!warning.is_empty()).then(|| warning.to_owned());
    if rejected_spans.is_none() && warning.is_none() {
        return OtlpPushOutcome::default();
    }
    OtlpPushOutcome {
        rejected_spans,
        warning,
    }
}

fn decode_otlp_success_response(
    content_type: Option<&str>,
    body: &[u8],
) -> std::result::Result<ExportTraceServiceResponse, String> {
    let content_type = content_type.map(str::trim).unwrap_or_default();
    let lowered = content_type.to_ascii_lowercase();
    if lowered.contains("application/x-protobuf") {
        return ExportTraceServiceResponse::decode(body)
            .map_err(|error| format!("invalid protobuf OTLP success body: {error}"));
    }
    if lowered.contains("application/json") {
        return serde_json::from_slice(body)
            .map_err(|error| format!("invalid JSON OTLP success body: {error}"));
    }

    match ExportTraceServiceResponse::decode(body) {
        Ok(response) => Ok(response),
        Err(protobuf_error) => match serde_json::from_slice(body) {
            Ok(response) => Ok(response),
            Err(json_error) => {
                if content_type.is_empty() {
                    Err(format!(
                        "unlabeled OTLP success body could not be decoded as protobuf ({protobuf_error}) or JSON ({json_error})"
                    ))
                } else {
                    Err(format!(
                        "unsupported OTLP success content-type {content_type:?}; protobuf decode failed ({protobuf_error}) and JSON decode failed ({json_error})"
                    ))
                }
            }
        },
    }
}

fn unreadable_otlp_success_warning(content_type: Option<&str>, body: &[u8], error: &str) -> String {
    let preview = std::str::from_utf8(body)
        .ok()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            const LIMIT: usize = 160;
            if text.chars().count() <= LIMIT {
                text.to_owned()
            } else {
                let truncated = text
                    .chars()
                    .take(LIMIT.saturating_sub(3))
                    .collect::<String>();
                format!("{truncated}...")
            }
        });

    match (
        content_type
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        preview,
    ) {
        (Some(content_type), Some(preview)) => format!(
            "collector returned unreadable success response ({content_type}): {error}; preview: {preview}"
        ),
        (Some(content_type), None) => {
            format!("collector returned unreadable success response ({content_type}): {error}")
        }
        (None, Some(preview)) => {
            format!("collector returned unreadable success response: {error}; preview: {preview}")
        }
        (None, None) => format!("collector returned unreadable success response: {error}"),
    }
}

fn normalize_otlp_endpoint(endpoint: Option<&str>) -> Result<Url, ApiError> {
    let raw = endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("PHOENIX_COLLECTOR_ENDPOINT").ok())
        .ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(
                "missing OTLP collector endpoint; pass one explicitly or set PHOENIX_COLLECTOR_ENDPOINT"
                    .to_owned(),
            )
        })?;
    let mut url = Url::parse(&raw)?;
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/v1/traces") {
        let next_path = if path.is_empty() {
            "/v1/traces".to_owned()
        } else {
            format!("{path}/v1/traces")
        };
        url.set_path(&next_path);
    }
    Ok(url)
}

fn resolved_otlp_headers(request: &PushOtlpExportRequest) -> Result<ReqwestHeaderMap, ApiError> {
    let mut headers = ReqwestHeaderMap::new();
    apply_env_otlp_headers(&mut headers)?;
    for (key, value) in &request.headers {
        insert_reqwest_header(&mut headers, key, value)?;
    }
    if let Some(api_key) = request
        .api_key
        .clone()
        .or_else(|| std::env::var("PHOENIX_API_KEY").ok())
        .filter(|value| !value.trim().is_empty())
    {
        if !headers.contains_key(REQWEST_AUTHORIZATION) {
            let bearer = format!("Bearer {api_key}");
            headers.insert(
                REQWEST_AUTHORIZATION,
                ReqwestHeaderValue::from_str(&bearer).map_err(|error| {
                    bt_core::BelltowerError::InvalidState(format!(
                        "invalid Phoenix API key for authorization header: {error}"
                    ))
                })?,
            );
        }
        if !headers.contains_key("api_key") {
            insert_reqwest_header(&mut headers, "api_key", &api_key)?;
        }
    }
    Ok(headers)
}

fn apply_env_otlp_headers(headers: &mut ReqwestHeaderMap) -> Result<(), ApiError> {
    let Some(raw) = std::env::var("PHOENIX_CLIENT_HEADERS").ok() else {
        return Ok(());
    };
    for pair in raw
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(bt_core::BelltowerError::InvalidState(format!(
                "invalid PHOENIX_CLIENT_HEADERS entry `{pair}`; expected key=value"
            ))
            .into());
        };
        insert_reqwest_header(headers, key.trim(), value.trim())?;
    }
    Ok(())
}

fn insert_reqwest_header(
    headers: &mut ReqwestHeaderMap,
    key: &str,
    value: &str,
) -> Result<(), ApiError> {
    let name = ReqwestHeaderName::from_bytes(key.trim().as_bytes()).map_err(|error| {
        bt_core::BelltowerError::InvalidState(format!("invalid OTLP header name `{key}`: {error}"))
    })?;
    let value = ReqwestHeaderValue::from_str(value.trim()).map_err(|error| {
        bt_core::BelltowerError::InvalidState(format!(
            "invalid OTLP header value for `{key}`: {error}"
        ))
    })?;
    headers.insert(name, value);
    Ok(())
}
