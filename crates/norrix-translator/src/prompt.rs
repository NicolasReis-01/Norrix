use anyhow::{Context, bail};
use indoc::indoc;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::{Language, TranslationContext, TranslationRequest};

pub(crate) fn prompts(request: &TranslationRequest) -> anyhow::Result<(String, String)> {
    let input = TranslationInput {
        source_language: request.source_language,
        target_language: request.target_language,
        context: &request.context,
        segments: request
            .segments
            .iter()
            .enumerate()
            .map(|(id, text)| TranslationInputSegment { id, text })
            .collect(),
    };
    let user = serde_json::to_string(&input).context("failed to serialize translation input")?;
    Ok((translation_system_prompt(request), user))
}

pub(crate) fn translations(
    provider: &str,
    text: &str,
    source_segments: &[String],
) -> anyhow::Result<Vec<String>> {
    let output = serde_json::from_str::<TranslationOutput>(text).with_context(|| {
        format!(
            "{provider} returned invalid translation JSON for {} segments",
            source_segments.len(),
        )
    })?;
    let mut translations = source_segments.to_vec();
    let mut translated = vec![false; source_segments.len()];

    for translation in output.translations {
        if translation.id >= translations.len() {
            bail!(
                "{provider} returned out-of-range segment ID {}; expected 0..{}",
                translation.id,
                translations.len()
            );
        }
        if translated[translation.id] {
            bail!(
                "{provider} returned duplicate segment ID {}",
                translation.id
            );
        }
        if translation.text.trim().is_empty() {
            bail!(
                "{provider} returned an empty translation for segment {} of {}",
                translation.id,
                source_segments.len()
            );
        }
        translations[translation.id] = translation.text;
        translated[translation.id] = true;
    }
    if let Some(missing) = translated.iter().position(|translated| !translated) {
        bail!(
            "{provider} returned an incomplete translation; segment {missing} of {} is missing",
            source_segments.len()
        );
    }

    Ok(translations)
}

pub(crate) fn output_schema(expected: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "translations": {
                "type": "array",
                "minItems": expected,
                "maxItems": expected,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": expected.saturating_sub(1),
                            "description": "The ID copied from the corresponding input segment."
                        },
                        "text": {
                            "type": "string",
                            "description": "The translation of the input segment with this ID."
                        }
                    },
                    "required": ["id", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["translations"],
        "additionalProperties": false
    })
}

fn translation_system_prompt(request: &TranslationRequest) -> String {
    let source = request
        .source_language
        .map(|language| language.to_string())
        .unwrap_or_else(|| "the detected source language".to_owned());
    let mut prompt = format!(
        indoc! {"
            You are a professional manga translator.

            Translation requirements:
            - Translate every input segment from {source} into natural {target}.
            - Preserve meaning, character voice, emotional tone, relationship nuance, emphasis, and sound effects.
            - Localize idioms and sound effects naturally while keeping wording concise enough for speech bubbles.
            - Use surrounding segments only for disambiguation and continuity; never merge or split segments.
            - Write every translated `text` value only in {target}; do not include source text, notes, explanations, or alternatives.
            - Never preserve or repeat original-language text; translate names, terms, and sound effects using natural {target} conventions.

            Output requirements:
            - Each input segment has a numeric `id`.
            - Return only a JSON object whose `translations` array contains one object with `id` and translated `text` for every input segment.
            - Copy every input ID exactly once; order does not matter.
            - Never merge, split, omit, duplicate, or add segments.
        "},
        source = source,
        target = request.target_language,
    )
    .trim_end()
    .to_owned();

    if !request.context.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(indoc! {"
            Context requirements:
            Use the supplied context only to preserve terminology, character voice, and dialogue continuity.
            Do not translate or return the context entries.
        "}.trim_end());
    }

    if request.image.is_some() {
        prompt.push_str("\n\n");
        prompt.push_str(indoc! {"
            Image requirements:
            Use the attached original page image as visual context for speaker identity, tone, layout, and ambiguous OCR.
            Translate only the supplied segments; do not add text seen in the image that is absent from the input segments.
        "}.trim_end());
    }

    if let Some(instructions) = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
    {
        prompt.push_str("\n\nAdditional instructions:\n");
        prompt.push_str(instructions);
    }
    prompt
}

#[derive(Serialize)]
struct TranslationInput<'a> {
    source_language: Option<Language>,
    target_language: Language,
    context: &'a [TranslationContext],
    segments: Vec<TranslationInputSegment<'a>>,
}

#[derive(Serialize)]
struct TranslationInputSegment<'a> {
    id: usize,
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct TranslationOutput {
    translations: Vec<TranslationOutputSegment>,
}

#[derive(Debug, Deserialize)]
struct TranslationOutputSegment {
    #[serde(deserialize_with = "deserialize_segment_id")]
    id: usize,
    text: String,
}

fn deserialize_segment_id<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SegmentId {
        Number(usize),
        String(String),
    }

    match SegmentId::deserialize(deserializer)? {
        SegmentId::Number(id) => Ok(id),
        SegmentId::String(id) => id.trim().parse().map_err(de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let source = ["one".to_owned(), "two".to_owned()];
        let response = r#"{"translations":[{"id":0,"text":"hello"},{"id":1,"text":"world"}]}"#;

        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn rejects_wrapped_and_malformed_json() {
        let source = ["one".to_owned(), "two".to_owned()];
        for response in [
            "```json\n{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"}]}\n```",
            r#"{translations: [{id: 0, text: 'hello'}, {id: 1, text: 'world'},],}"#,
            r#"Here is the result: {"translations": [{"id": 0, "text": "hello"}, {"id": 1, "text": "world"},]}"#,
            "{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"",
        ] {
            assert!(
                translations("test", response, &source).is_err(),
                "{response}"
            );
        }
    }

    #[test]
    fn parse_error_preserves_the_root_failure_without_logging_response_content() {
        let source = ["one".to_owned()];
        let response =
            "{\n  \"private dialogue\": \"do not log\", \"translations\": [{\"id\": 0}]\n}";
        let error = format!(
            "{:#}",
            translations("test", response, &source).expect_err("missing text should fail")
        );

        assert!(error.contains("missing field `text`"), "{error}");
        assert!(
            error.contains("invalid translation JSON for 1 segments"),
            "{error}"
        );
        assert!(!error.contains("do not log"), "{error}");
    }

    #[test]
    fn restores_input_order_from_ids() {
        let source = ["one".to_owned(), "two".to_owned()];
        let response = r#"{"translations":[{"id":1,"text":"world"},{"id":0,"text":"hello"}]}"#;
        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn rejects_duplicate_and_out_of_range_segment_ids() {
        let source = ["one".to_owned(), "two".to_owned()];
        let short = r#"{"translations":[{"id":1,"text":"world"}]}"#;
        assert!(
            translations("test", short, &source)
                .unwrap_err()
                .to_string()
                .contains("segment 0")
        );

        let duplicate = concat!(
            r#"{"translations":["#,
            r#"{"id":0,"text":"hello"},"#,
            r#"{"id":0,"text":"duplicate"},"#,
            r#"{"id":1,"text":"world"}"#,
            "]}"
        );
        assert!(
            translations("test", duplicate, &source)
                .unwrap_err()
                .to_string()
                .contains("duplicate segment ID")
        );
        let out_of_range = r#"{"translations":[{"id":0,"text":"hello"},{"id":1,"text":"world"},{"id":9,"text":"extra"}]}"#;
        assert!(
            translations("test", out_of_range, &source)
                .unwrap_err()
                .to_string()
                .contains("out-of-range segment ID")
        );
    }

    #[test]
    fn rejects_empty_translation_segments() {
        let source = ["one".to_owned()];
        let response = r#"{"translations":[{"id":0,"text":"  \n"}]}"#;
        assert!(
            translations("test", response, &source)
                .unwrap_err()
                .to_string()
                .contains("empty translation")
        );
    }

    #[test]
    fn prompt_payload_contains_ordered_context() {
        let request = TranslationRequest::new(["new"], Language::English)
            .with_context([TranslationContext::new("old", "previous")]);
        let (_, user) = prompts(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&user).unwrap();
        assert_eq!(input["context"][0]["source"], "old");
        assert_eq!(input["context"][0]["translation"], "previous");
        assert_eq!(input["segments"][0]["id"], 0);
        assert_eq!(input["segments"][0]["text"], "new");
    }

    #[test]
    fn system_prompt_encodes_invariants_and_custom_instructions() {
        let request = TranslationRequest::new(["hello"], Language::Korean)
            .with_source_language(Language::Japanese)
            .with_instructions("Use informal speech.");
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("from Japanese into natural Korean"));
        assert!(prompt.contains("Copy every input ID exactly once"));
        assert!(prompt.contains("Use informal speech."));
    }

    #[test]
    fn schema_requires_the_expected_number_of_id_text_pairs() {
        let schema = output_schema(3);
        let translations = &schema["properties"]["translations"];
        assert_eq!(translations["minItems"], 3);
        assert_eq!(translations["maxItems"], 3);
        assert_eq!(translations["items"]["properties"]["id"]["minimum"], 0);
        assert_eq!(translations["items"]["properties"]["id"]["maximum"], 2);
        assert_eq!(translations["items"]["additionalProperties"], false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn empty_custom_instructions_are_ignored() {
        let request = TranslationRequest::new(["hello"], Language::English).with_instructions("  ");
        assert!(!translation_system_prompt(&request).contains("Additional instructions"));
    }

    #[test]
    fn context_is_reference_only() {
        let request = TranslationRequest::new(["Where is she?"], Language::Japanese)
            .with_context([TranslationContext::new("I saw Alice.", "アリスを見た。")]);
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("dialogue continuity"));
        assert!(prompt.contains("Do not translate or return the context"));
    }

    #[test]
    fn image_context_does_not_expand_the_translation_scope() {
        let request = TranslationRequest::new(["text"], Language::English)
            .with_image(std::sync::Arc::new(image::DynamicImage::new_rgb8(1, 1)));
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("attached original page image"));
        assert!(prompt.contains("Translate only the supplied segments"));
    }
}
