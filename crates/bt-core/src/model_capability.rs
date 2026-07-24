//! Centralized model capability checks.
//!
//! Every "does model X support Y" predicate that more than one crate needs
//! lives here, so capability tables cannot fork per crate. bt-context uses
//! these to pick request defaults; bt-providers uses them to keep wire
//! requests inside what each endpoint actually accepts.

use crate::types::ThinkingEffort;

/// Models in the OpenAI reasoning family accept a `reasoning_effort` request
/// parameter and take their completion budget via `max_completion_tokens`.
#[must_use]
pub fn openai_reasoning_family(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("gpt-5")
}

/// Default reasoning effort to request when the operator did not set one.
///
/// `xhigh` is only recognized for codex-max variants; newer general gpt-5.x
/// releases reject it (observed: HTTP 400 from chat completions for
/// gpt-5.4 with `reasoning_effort: "xhigh"`). Everything else in the
/// reasoning family defaults to `high`.
#[must_use]
pub fn openai_default_reasoning_effort(model: &str) -> Option<ThinkingEffort> {
    if !openai_reasoning_family(model) {
        return None;
    }
    if model.to_ascii_lowercase().contains("codex-max") {
        return Some(ThinkingEffort::XHigh);
    }
    Some(ThinkingEffort::High)
}

/// The OpenAI chat-completions endpoint accepts `reasoning_effort` values up
/// to `high`; `xhigh` is only understood by the Codex/Responses backends.
/// Clamp rather than let the request fail with HTTP 400.
#[must_use]
pub fn clamp_openai_chat_reasoning_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::XHigh => ThinkingEffort::High,
        other => other,
    }
}

/// Anthropic models with extended thinking support: the claude-4 and claude-5
/// generations (opus/sonnet/haiku/fable/mythos and future family names).
/// Parses the first numeric segment after the `claude-` prefix so new family
/// names do not silently lose thinking.
#[must_use]
pub fn anthropic_supports_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    let Some(rest) = model.strip_prefix("claude-") else {
        return false;
    };
    rest.split('-')
        .find_map(|segment| segment.parse::<u32>().ok())
        .is_some_and(|major| major >= 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_family_covers_o_series_and_gpt5() {
        assert!(openai_reasoning_family("o3-mini"));
        assert!(openai_reasoning_family("gpt-5"));
        assert!(openai_reasoning_family("gpt-5.4-mini"));
        assert!(openai_reasoning_family("GPT-5.1-codex-max"));
        assert!(!openai_reasoning_family("gpt-4o"));
        assert!(!openai_reasoning_family("qwen3:latest"));
    }

    #[test]
    fn default_effort_is_high_except_codex_max() {
        assert_eq!(
            openai_default_reasoning_effort("gpt-5.4-mini"),
            Some(ThinkingEffort::High)
        );
        assert_eq!(
            openai_default_reasoning_effort("gpt-5.4"),
            Some(ThinkingEffort::High)
        );
        assert_eq!(
            openai_default_reasoning_effort("gpt-5.1"),
            Some(ThinkingEffort::High)
        );
        assert_eq!(
            openai_default_reasoning_effort("gpt-5.1-codex-max"),
            Some(ThinkingEffort::XHigh)
        );
        assert_eq!(
            openai_default_reasoning_effort("o3"),
            Some(ThinkingEffort::High)
        );
        assert_eq!(openai_default_reasoning_effort("gpt-4o"), None);
    }

    #[test]
    fn chat_completions_clamp_downgrades_only_xhigh() {
        assert_eq!(
            clamp_openai_chat_reasoning_effort(ThinkingEffort::XHigh),
            ThinkingEffort::High
        );
        assert_eq!(
            clamp_openai_chat_reasoning_effort(ThinkingEffort::Low),
            ThinkingEffort::Low
        );
    }

    #[test]
    fn anthropic_thinking_covers_claude_4_and_5_generations() {
        assert!(anthropic_supports_thinking("claude-opus-4-8"));
        assert!(anthropic_supports_thinking("claude-sonnet-4-6"));
        assert!(anthropic_supports_thinking("claude-haiku-4-5-20251001"));
        assert!(anthropic_supports_thinking("claude-sonnet-5"));
        assert!(anthropic_supports_thinking("claude-fable-5"));
        assert!(!anthropic_supports_thinking("claude-3-5-sonnet-latest"));
        assert!(!anthropic_supports_thinking("gpt-5.4"));
    }
}
