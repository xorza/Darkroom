use crate::normalize_string::NormalizeString;

pub fn reformat_yaml(yaml: &str) -> anyhow::Result<String> {
    let value = serde_yaml::from_str::<serde_yaml::Value>(yaml)?;
    let yaml = serde_yaml::to_string(&value)?;
    let yaml = yaml.normalize();
    Ok(yaml)
}
