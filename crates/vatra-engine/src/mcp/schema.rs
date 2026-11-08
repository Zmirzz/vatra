use serde_json::Value;

/// JSON Schema allows `true` for "anything", but llama.cpp rejects the whole request over it.
pub fn normalize_schema(schema: Value) -> Value {
    match schema {
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| {
                    let value = match (key.as_str(), value) {
                        ("properties", Value::Object(properties)) => Value::Object(
                            properties.into_iter().map(|(name, inner)| (name, normalize_subschema(inner))).collect(),
                        ),
                        ("items", inner) => normalize_subschema(inner),
                        (_, inner) => normalize_schema(inner),
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_schema).collect()),
        other => other,
    }
}

fn normalize_subschema(schema: Value) -> Value {
    match schema {
        Value::Bool(true) => Value::Object(Default::default()),
        other => normalize_schema(other),
    }
}
