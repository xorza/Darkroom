use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Yaml,
    Json,
}

impl FileFormat {
    pub fn from_file_name(file_name: &str) -> anyhow::Result<Self> {
        let extension = Path::new(file_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());

        match extension.as_deref() {
            Some("yaml") | Some("yml") => Ok(Self::Yaml),
            Some("json") => Ok(Self::Json),
            _ => Err(anyhow::anyhow!(
                "Unsupported file extension for file: {}",
                file_name
            )),
        }
    }
}
