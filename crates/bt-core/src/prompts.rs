//! Harness-owned prompt text for runtime-internal LLM calls.
//!
//! These are not operator instructions: they drive recorded context
//! maintenance work (for example the compaction summarization call) and are
//! versioned with the code so replay can attribute prompt text to a build.

/// Reserved `llm_call_ordinal` for runtime context-maintenance completions
/// (currently the compaction summarization call). Turn-loop provider calls
/// start at ordinal 1, so ordinal 0 never collides with them and consumers
/// can distinguish maintenance calls from model turn traffic. Observed
/// context-size accounting skips ordinal-0 completions because their usage
/// describes the summarization input, not the session context.
pub const CONTEXT_MAINTENANCE_LLM_CALL_ORDINAL: u32 = 0;

/// System prompt for the compaction summarization completion.
///
/// Derived from codex's context-checkpoint compaction structure (progress,
/// decisions, constraints, remaining work, critical data) plus letta's
/// additions: identifiers preserved verbatim and a final "Lookup hints"
/// section whose terms feed session search over evicted history.
pub const COMPACTION_SUMMARY_PROMPT: &str = "\
You are performing a context checkpoint compaction for an ongoing agent \
session. Older messages are being evicted from the model-visible context; \
write a handoff summary so another LLM can continue the session without \
them. The full history remains durably recorded and searchable, so your \
summary is an index and checkpoint, not the only copy.

Structure the summary with exactly these sections:

1. Current progress: what has been accomplished so far, in order.
2. Key decisions: decisions made and the reasons behind them.
3. Constraints and user preferences: requirements, rules, and preferences \
the user has expressed or the work must respect.
4. What remains: unfinished work and concrete next steps.
5. Critical data and references: exact values that must survive verbatim.

Rules:
- Preserve identifiers verbatim (file paths, URLs, ids, commands, error \
codes, names, numbers). Never paraphrase an identifier.
- If a previous compaction summary is included in the input, fold its \
still-relevant content into this summary; it will not be retained \
separately.
- Be factual. Do not invent content that is not present in the input.
- Write plain text only: no markdown tables, no code fences.

End with a final section titled \"Lookup hints\": a short list of key terms \
and topics that could be used to find the evicted material in session \
history search later.";
