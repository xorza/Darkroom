use crate::normalize_string::NormalizeString;

pub fn reformat_yaml(yaml: &str) -> anyhow::Result<String> {
    let value = serde_yml::from_str::<serde_yml::Value>(yaml)?;
    let yaml = serde_yml::to_string(&value)?;
    let yaml = yaml.normalize();
    Ok(yaml)
}
