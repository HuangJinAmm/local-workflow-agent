use serde_json::Value;

pub(crate) fn merge_openai_compatible_options(body: &mut Value, provider_options: &Value) {
    let Some(options_obj) = provider_options.as_object() else {
        return;
    };

    for (key, value) in options_obj {
        match key.as_str() {
            "reasoningEffort" => body["reasoning_effort"] = value.clone(),
            "textVerbosity" => body["verbosity"] = value.clone(),
            _ => body[key] = value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_openai_compatible_maps_reasoning_fields() {
        let mut body = json!({});
        merge_openai_compatible_options(
            &mut body,
            &json!({
                "reasoningEffort": "high",
                "textVerbosity": "low",
                "store": false,
            }),
        );

        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(body["verbosity"], json!("low"));
        assert_eq!(body["store"], json!(false));
    }
}
