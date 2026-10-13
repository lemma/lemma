use indexmap::IndexMap;
use lemma::EvaluateDataValue;

/// Parse `application/x-www-form-urlencoded` fields into data inputs (all [`EvaluateDataValue::Literal`] values).
pub fn form_urlencoded_to_data_values(
    body: &[u8],
) -> Result<IndexMap<String, EvaluateDataValue>, String> {
    let fields: IndexMap<String, String> =
        serde_urlencoded::from_bytes(body).map_err(|e| format!("invalid form body: {e}"))?;
    Ok(fields
        .into_iter()
        .map(|(k, v)| (k, EvaluateDataValue::string(v)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_urlencoded_parsed_as_string_input() {
        let map = form_urlencoded_to_data_values(b"code=AD&quantity=3").unwrap();
        assert_eq!(
            map.get("code"),
            Some(&EvaluateDataValue::Literal("AD".to_string()))
        );
        assert_eq!(
            map.get("quantity"),
            Some(&EvaluateDataValue::Literal("3".to_string()))
        );
    }

    #[test]
    fn form_urlencoded_decodes_plus_and_percent() {
        let map = form_urlencoded_to_data_values(b"name=hello+world&city=S%C3%A3o+Paulo").unwrap();
        assert_eq!(
            map.get("name"),
            Some(&EvaluateDataValue::Literal("hello world".to_string()))
        );
        assert_eq!(
            map.get("city"),
            Some(&EvaluateDataValue::Literal("São Paulo".to_string()))
        );
    }
}
