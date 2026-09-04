use serde_json::Value;

pub(super) fn validate_json_shape(value: &Value, schema: &Value) -> Result<(), ()> {
    if let Some(alternatives) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        return alternatives
            .iter()
            .any(|candidate| validate_json_shape(value, candidate).is_ok())
            .then_some(())
            .ok_or(());
    }
    if let Some(types) = schema.get("type") {
        let valid = types.as_str().map_or_else(
            || {
                types
                    .as_array()
                    .map(|values| {
                        values.iter().any(|entry| {
                            entry
                                .as_str()
                                .is_some_and(|kind| json_type_matches(value, kind))
                        })
                    })
                    .unwrap_or(true)
            },
            |kind| json_type_matches(value, kind),
        );
        if !valid {
            return Err(());
        }
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.iter().any(|candidate| candidate == value))
    {
        return Err(());
    }
    if schema
        .get("const")
        .is_some_and(|constant| constant != value)
    {
        return Err(());
    }
    validate_object(value, schema)?;
    validate_array(value, schema)?;
    if let (Some(max), Some(string)) = (
        schema.get("maxLength").and_then(Value::as_u64),
        value.as_str(),
    ) {
        if string.encode_utf16().count() as u64 > max {
            return Err(());
        }
    }
    Ok(())
}

fn validate_object(value: &Value, schema: &Value) -> Result<(), ()> {
    let (Some(object), Some(properties)) = (
        value.as_object(),
        schema.get("properties").and_then(Value::as_object),
    ) else {
        return Ok(());
    };
    if schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .any(|key| key.as_str().is_some_and(|key| !object.contains_key(key)))
        })
    {
        return Err(());
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false))
        && object.keys().any(|key| !properties.contains_key(key))
    {
        return Err(());
    }
    for (key, item) in object {
        if let Some(property_schema) = properties.get(key) {
            validate_json_shape(item, property_schema)?;
        }
    }
    Ok(())
}

fn validate_array(value: &Value, schema: &Value) -> Result<(), ()> {
    let Some(array) = value.as_array() else {
        return Ok(());
    };
    if schema
        .get("maxItems")
        .and_then(Value::as_u64)
        .is_some_and(|max| array.len() as u64 > max)
    {
        return Err(());
    }
    if let Some(items) = schema.get("items") {
        for item in array {
            validate_json_shape(item, items)?;
        }
    }
    Ok(())
}

fn json_type_matches(value: &Value, kind: &str) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}
