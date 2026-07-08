# Provider Output and Transcript Model

This document defines the target model for how Belltower should represent model output.

It exists because the current architecture is stronger than the current content algebra:

- the canonical event store is principled
- raw chunk durability is strong
- turn and trace modeling are strong
- but the normalized provider-output and transcript-content types are still too narrow

Today Belltower mainly models:

- visible text
- tool calls
- tool results
- usage
- finish reasons
- raw chunks

That is enough for basic coding-agent turns, but it is not enough for the long-term harness goal.
Reasoning, refusals, structured output, citations, and media need a coherent place in the model.

This document is the target shape for that work.

It is informed by local review of:

- `pi-mono`
- Hermes
- Codex-rs
- OpenCode

## Design Goals

The target model should be:

- clean
- typed
- simple enough to reason about
- rich enough to cover real provider behavior
- scalable for long sessions and replay
- explicit about what is transcript content versus transport evidence
- compatible with the canonical event store and raw chunk durability model

Backward compatibility is not a design constraint for this change.
If the right model requires breaking `MessageContent`, `CompletionChunk`, projections, and TUI rendering code, that is acceptable.

## The Core Split

Belltower should make three layers explicit:

1. raw transport evidence
2. normalized provider-output deltas
3. canonical transcript content

Those layers are related, but they are not the same thing.

### 1. Raw Transport Evidence

This stays exactly what Belltower already treats it as:

- provider-native stream data
- durable
- queryable
- not directly the user transcript

Raw chunks remain the lossless evidence layer used for:

- parser debugging
- replay
- fidelity audits
- self-inspection
- future dataset generation

### 2. Normalized Provider-Output Deltas

This is the incremental model returned by providers while a turn is streaming.

It should represent what the provider is currently emitting in a provider-agnostic way:

- text deltas
- reasoning deltas
- tool-call construction
- refusal content
- structured output
- usage snapshots

Citations and media remain part of the longer-term architectural target, but they are explicitly deferred from the first migration slice.

This layer is for streaming and accumulation.
It is not the same thing as a finalized message.

### 3. Canonical Transcript Content

This is what ends up in `message.appended` and transcript projections.

It should represent finalized content parts in stable order:

- visible answer text
- reasoning content when the provider exposed it
- finalized tool calls
- tool results
- refusals
- structured output

Citations and media references remain part of the architectural target, but not the first implementation slice.

This is the content a human or agent inspects as the logical turn transcript.

Compaction summaries are model-visible context, but they are not assistant
answers. When history is compacted, `bt-context` creates a synthetic system
summary message, and `bt-runtime` records the durable `context.compacted` event
plus a context-manifest attachment that references the same compaction id. That
keeps transcript display, context mutation, replay, and export distinct while
still making summarized context inspectable.

## Target Provider-Output Model

The current `CompletionChunk` shape:

- `delta_text`
- `delta_tool_call`
- `usage`
- `raw`

should be replaced with a typed delta model.

The important design rule is:

- one chunk may contain zero, one, or multiple normalized deltas
- usage can still arrive independently
- raw transport payload can still be attached to the chunk

The implemented first-slice shape is:

```rust
pub struct CompletionChunk {
    pub llm_call_ordinal: Option<u32>,
    pub deltas: Vec<CompletionDelta>,
    pub usage: Option<TokenUsage>,
    pub raw: Option<Value>,
}

pub enum CompletionDelta {
    AppendText {
        text: String,
    },
    AppendReasoning {
        text: Option<String>,
        redacted: bool,
        opaque_replay: Option<Value>,
    },
    OpenToolCall {
        call_id: String,
        tool_name: String,
        arguments: Option<Value>,
    },
    AppendToolCallArguments {
        call_id: String,
        partial_json: String,
    },
    CloseToolCall {
        call_id: String,
    },
    AppendRefusal {
        text: Option<String>,
        provider_reason: Option<String>,
        opaque_metadata: Option<Value>,
    },
    SetStructuredOutput {
        schema_name: Option<String>,
        value: Value,
    },
}
```

This keeps the model simple:

- text-like things stream as append-only deltas
- tool calls have explicit open/append/close semantics
- structured output can land atomically
- refusal and reasoning deltas can preserve provider detail without being flattened into plain text

It is also compatible with current Belltower strengths:

- chunks can still be committed incrementally
- chunks can still be mirrored into `completion.chunk`
- raw chunks remain separate evidence

## Target Transcript Model

The current `MessageContent` shape:

- `Text`
- `ToolCall`
- `ToolResult`
- `Multipart`

should be replaced with message parts.

The important design rule is:

- transcript messages are finalized content
- transcript messages may contain multiple ordered parts
- generic `Multipart` should disappear

The implemented first-slice shape is:

```rust
pub struct Message {
    pub message_id: MessageId,
    pub role: Role,
    pub parts: Vec<MessagePart>,
    pub created_at: OffsetDateTime,
}

pub enum MessagePart {
    Text {
        text: String,
    },
    Reasoning {
        text: Option<String>,
        redacted: bool,
        opaque_replay: Option<Value>,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolResult {
        result: ToolResultEnvelope,
    },
    Refusal {
        text: Option<String>,
        provider_reason: Option<String>,
        opaque_metadata: Option<Value>,
    },
    Structured {
        schema_name: Option<String>,
        value: Value,
    },
}
```

The exact fields can still evolve, but the important thing is the stable part taxonomy and ordered `parts` model.

### Why This Is Better Than `Multipart`

`Multipart` is convenient but weak:

- it pushes meaning into ad hoc JSON
- it makes projection and rendering logic branchy
- it weakens event/export semantics
- it becomes a dumping ground once providers get more complex

Belltower should prefer explicit parts and use raw chunks as the escape hatch for unsupported provider detail.
That is cleaner than pretending unsupported provider structure is transcript structure.

## What Counts As Transcript Content

The transcript should store:

- assistant-visible text
- provider-exposed reasoning content
- finalized tool calls
- tool results
- refusals
- structured output that matters to the turn result

The transcript should not store:

- transport-only noise
- provider-specific SSE framing details
- metadata that is better represented as event fields

Usage, finish reason, latency, and cost remain turn or completion metadata, not message parts.

## Part Ordering And Coalescing

Transcript parts should preserve provider order, but they should not preserve transport-level fragmentation.

The rule is:

- preserve part ordering
- merge adjacent parts of the same logical type
- do not merge across different logical types

Examples:

- `Text("Hel")` followed by `Text("lo")` becomes one `Text("Hello")`
- `Reasoning("step 1")` followed by `Reasoning(" step 2")` becomes one `Reasoning(...)`
- `Reasoning(...)`, then `Text(...)`, then `Reasoning(...)` stays three parts

Why this rule exists:

- raw chunks already preserve exact transport granularity
- normalized completion deltas already preserve streaming behavior
- transcript content should represent logical message structure, not token-by-token fragmentation

This keeps transcript storage, rendering, export, and diffing much simpler without losing raw evidence.

## Reasoning Model

Reasoning needs special treatment because providers differ.

There are three real cases:

1. no reasoning exposure
2. reasoning token counts only
3. provider-exposed reasoning content

Belltower should model that explicitly.

Reasoning content in the transcript should only exist when the provider actually exposed it.
Belltower must not synthesize hidden chain-of-thought.

The recommended content model for reasoning is:

- `ReasoningPart` when text or structured reasoning content is exposed
- `TokenUsage.reasoning_tokens` when only counts are exposed
- raw chunk evidence for provider-native details or replay blobs

Reasoning content should be stored canonically but hidden by default in the TUI.
That matches the best operator pattern from Hermes, `pi-mono`, and Codex-rs:

- reasoning is part of the record
- reasoning display is a local UI policy

For now, Belltower should use one reasoning transcript type rather than splitting transcript content into `ReasoningSummary` and `ReasoningContent`.

That keeps the transcript model simpler.
If later providers or operator needs justify it, Belltower can still distinguish those cases in:

- normalized delta variants
- event attributes
- raw chunk evidence

without forcing an early transcript-level split.

The implemented first-slice transcript shape for reasoning is:

```rust
Reasoning {
    text: Option<String>,
    redacted: bool,
    opaque_replay: Option<Value>,
}
```

That is enough to cover:

- visible provider-exposed reasoning
- provider-exposed but redacted reasoning
- replay blobs or opaque continuation payloads where the provider requires them

while staying substantially simpler than the Codex-rs split between reasoning summary and raw reasoning content.

## Refusal Model

Refusal should not be treated as display text only.

Frontier APIs increasingly model refusal as structured state:

- OpenAI structured outputs expose refusal separately from schema-shaped output
- Anthropic may signal refusal as a stop condition even when the visible text is limited or absent

So Belltower should keep refusal as a first-class transcript/content concept with room for metadata.

The implemented first-slice transcript shape is:

```rust
Refusal {
    text: Option<String>,
    provider_reason: Option<String>,
    opaque_metadata: Option<Value>,
}
```

This lets Belltower preserve:

- a user-visible refusal message when present
- provider refusal classification when present
- provider-specific refusal details without forcing them into the main type surface

That is better than collapsing refusals into plain `Text`, because refusals are semantically different from normal assistant output.

## Structured Output Model

Structured output should start simple.

On the request side, Belltower should carry an explicit structured-output request contract rather than relying on prompt conventions alone.

The current first implementation shape is:

```rust
StructuredOutputSpec {
    schema_name: Option<String>,
    schema: Value,
}
```

and `CompletionRequest` may carry `structured_output: Option<StructuredOutputSpec>`.

`CompletionRequest` also carries quiet provider-thinking hints through
`thinking: Option<ThinkingConfig>`. This is request-side capability policy, not
transcript content. The context/request layer may default known reasoning-capable
models to the strongest safe effort while provider adapters translate that
intent into OpenAI, ChatGPT/Codex, or Anthropic-specific request fields. Local
OpenAI-compatible backends are not opted in solely because their wire protocol
resembles OpenAI.

That request-side contract is intentionally narrower than a full provider-capability system:

- it lets providers opt in explicitly
- it keeps schema ownership in Belltower types
- it avoids pretending every provider can satisfy schema-constrained output the same way
- it avoids turning provider-specific reasoning controls into user-facing
  settings before there is a proven operator need

The current provider support is deliberately limited:

- hosted `openai` requests can attach a JSON-schema response format and emit a final `SetStructuredOutput` delta
- `anthropic` currently rejects structured-output requests explicitly rather than silently degrading
- structured output and tool calling are not yet combined in one request path

That is acceptable for now because it gives Belltower one truthful structured-output path before generalizing across providers.

The important product-scoping rule is:

- structured output stays in the canonical request and transcript model
- but it is not currently a primary operator surface
- Belltower should not present schema-constrained output as a headline interactive feature until the auth/model/operator flow is sharper
- current support should remain quiet and truthful: available in the core model, supported where implemented, and otherwise explicitly unsupported

The recommended transcript shape is:

```rust
Structured {
    schema_name: Option<String>,
    value: Value,
}
```

This is intentionally generic:

- the transcript records the structured artifact
- optional schema identity can be attached when available
- Belltower avoids prematurely baking one schema system into its core message model

If later integrations need schema validation or strongly typed decoded views, those can be layered on top of this canonical representation.

## Citation And Media Model

Citation and media are still part of the architectural target, but they are deferred from the first migration slice.

When Belltower adds them, the transcript representation should stay reference-oriented and compact.

By media here, Belltower means things like:

- image URLs
- local file paths
- artifact IDs
- MIME type hints
- optional captions or titles

It does not mean embedding large image/audio/document payloads directly inside transcript rows.

The recommended eventual transcript shape is:

```rust
MediaRef {
    kind: String,
    location: String,
    mime_type: Option<String>,
    caption: Option<String>,
}
```

Large bytes should live in:

- raw chunks
- artifact storage
- future file or blob stores

not in the canonical transcript projection.

This is deferred because Belltower does not yet have the full artifact/provenance path needed to make media references semantically meaningful instead of operationally ad hoc.

## Capability Model

Not every connection or model supports every output kind.

Belltower should therefore add a capability model for provider/model behavior, for example:

- tool calling
- reasoning token counts
- reasoning content
- reasoning replay blobs
- citations
- structured output
- image input
- image output

These capabilities belong in readiness/model inspection surfaces, not in the transcript itself.

This will matter for:

- `/use`
- `/model`
- `/models`
- provider integration tests

## Event Model Implications

The event taxonomy does not need a fundamental redesign.

The important changes are:

- `completion.chunk` should carry typed normalized deltas instead of only `delta_text` and `delta_tool_call`
- `message.appended` should carry the richer `Message` with ordered parts
- `raw_chunk.persisted` stays as the transport evidence marker

That means the existing canonical sequence remains valid:

- `completion.requested`
- `completion.chunk`
- `completion.finished`
- `message.appended`
- `turn.finished`

The payloads get richer; the event families do not need to proliferate.

## Storage And Projection Implications

This model implies:

- replacing serialized `MessageContent` with serialized `Vec<MessagePart>`
- replacing serialized `CompletionChunk` payload shape in events
- rebuilding message and transcript projections
- updating raw-to-structured diffing to understand richer delta types

Because backward compatibility is not a priority right now, Belltower should not carry dual-format compatibility shims for long.
Prefer:

- one explicit schema migration
- one explicit projection rebuild
- one explicit event-payload version shift in active development

## TUI Implications

The TUI should render the richer transcript model directly.

Recommended default policy:

- `Text` shown normally
- `ToolCall` and `ToolResult` shown as today
- `Reasoning` hidden from the default immutable transcript view
- `Refusal` rendered distinctly
- `Structured` shown when relevant but not noisy
- citations and media can be added later without redesigning the transcript model

For the current shell-style TUI, richer detail should come through explicit drill-down surfaces such as `inspect`, not by rerendering historical transcript output in place.

## What Belltower Should Borrow

From `pi-mono`:

- typed content parts
- typed streaming event algebra

From OpenCode:

- compact message-part design
- incremental message updates from provider events

From Codex-rs:

- explicit separation between transcript items and lower-level runtime events
- richer reasoning distinction between summary and raw content

From Hermes:

- reasoning should be stored but hidden by default in the operator UI

## What Belltower Should Not Borrow

Belltower should not make OpenAI-style message dicts the canonical internal model.

That Hermes-style approach is flexible, but it is weaker for:

- projection-backed reads
- typed rendering
- cross-provider normalization
- telemetry alignment
- replay safety

Belltower is better served by explicit enums and typed content parts.

## Recommended Implementation Order

1. Replace `MessageContent` with `Vec<MessagePart>`.
2. Replace the current `CompletionChunk` shape with typed `CompletionDelta`s.
3. Update OpenAI and Anthropic providers to emit the richer delta model.
4. Update `completion.chunk` canonical event payloads to carry typed deltas.
5. Update message projections, `/raw diff`, and OTEL/export mapping.
6. Add reasoning-aware inspection and transcript rendering without mutating printed history.
7. Add more provider-output kinds later:
   - citations
   - media output
   - richer refusal rendering and policy

## Immediate Scope

The first implementation slice does not need every possible provider feature.

The minimum coherent step is:

- `Text`
- `Reasoning`
- `ToolCall`
- `ToolResult`
- `Refusal`
- `Structured`
- typed `CompletionDelta`

That gets Belltower onto a much stronger model without over-designing citations and media before the providers using them are in place.

`MediaRef` remains part of the architectural target, but it should be deferred from the first migration until Belltower has a clearer artifact/reference path for inline media outputs.

That is not because media is unimportant.
It is because the right long-term role for media in Belltower is larger than "the model returned some bytes."

The intended direction is:

- raw chunks preserve provider-native image/audio/file payloads or handles
- transcript content eventually references canonical artifacts rather than embedding heavy blobs
- artifact or provenance layers can later express semantic relationships such as:
  - this image was produced by this turn
  - this later output derived from that earlier artifact
  - this child session consumed that artifact
  - these artifacts belong to one workflow lineage

That aligns better with Belltower's long-term thesis around canonical telemetry, workflow lineage, and self-inspection than treating media as an opaque blob in transcript rows.
