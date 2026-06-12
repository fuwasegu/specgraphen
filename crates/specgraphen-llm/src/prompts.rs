use crate::{LlmMessage, LlmRequest, MessageRole, ResponseFormat};

pub fn behavior_extraction_prompt(
    entity_label: &str,
    entity_type: &str,
    source_code: &str,
    context_snippets: &[String],
) -> LlmRequest {
    let system = r#"You are a precise Java code analyzer. Your task is to extract behavioral semantics from Java code entities.

RULES:
1. Every claim MUST cite specific line number(s) from the source code.
2. Claims without line citations will be REJECTED.
3. Be factual and precise. Do not speculate beyond what the code shows.
4. Return valid JSON only.

Output JSON schema:
{
  "intent": "one-line purpose description",
  "behavior": "step-by-step behavioral description",
  "preconditions": ["condition: description (line X)"],
  "postconditions": ["condition: description (line X)"],
  "side_effects": ["effect description (line X)"],
  "error_behavior": "how errors are handled (lines X-Y)",
  "witnesses": [{"claim": "the claim text", "lines": [start_line, end_line]}]
}"#;

    let context_block = if context_snippets.is_empty() {
        String::new()
    } else {
        format!("\n\n## Related code:\n{}", context_snippets.join("\n---\n"))
    };

    let user_msg = format!(
        "Analyze this Java {entity_type} `{entity_label}`:\n\n```java\n{source_code}\n```{context_block}"
    );

    LlmRequest {
        system_prompt: system.to_string(),
        messages: vec![LlmMessage {
            role: MessageRole::User,
            content: user_msg,
        }],
        max_tokens: 2048,
        temperature: 0.0,
        response_format: Some(ResponseFormat::JsonObject),
    }
}
