use crate::normalize_string::NormalizeString;

pub fn reformat_yaml(yaml: &str) -> anyhow::Result<String> {
    let value = serde_yml::from_str::<serde_yml::Value>(yaml)?;
    Ok(serde_yml::to_string(&value)?.normalize())
}
