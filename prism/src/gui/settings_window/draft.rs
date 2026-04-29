//! Editable mirror of [`Config`] for the Settings window. Validation
//! lives in [`SettingsDraft::to_config`]; the GUI never touches the
//! persisted config until Apply succeeds.
//!
//! `tcp` survives toggling `tcp_enabled` off so a misclick doesn't
//! destroy whatever the user typed; only `tcp_enabled = true` writes
//! `script_tcp = Some(..)` on apply.

use std::net::SocketAddr;
use std::path::PathBuf;

use uuid::Uuid;

use crate::config::Config;
use crate::script::TcpScriptConfig;

#[derive(Debug, Clone)]
pub struct SettingsDraft {
    pub load_last: bool,
    pub tcp_enabled: bool,
    pub tcp: TcpDraft,
    /// Preserved verbatim across Apply — not user-editable from the
    /// settings window.
    pub current_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TcpDraft {
    pub bind_text: String,
    pub no_auth: bool,
    pub token: Uuid,
    pub token_file_text: String,
}

impl Default for TcpDraft {
    fn default() -> Self {
        Self {
            bind_text: "127.0.0.1:0".to_string(),
            no_auth: false,
            token: Uuid::new_v4(),
            token_file_text: String::new(),
        }
    }
}

impl TcpDraft {
    fn from_config(cfg: &TcpScriptConfig) -> Self {
        Self {
            bind_text: cfg.bind.to_string(),
            no_auth: cfg.token.is_none(),
            token: cfg.token.unwrap_or_else(Uuid::new_v4),
            token_file_text: cfg
                .token_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }

    fn to_config(&self) -> Result<TcpScriptConfig, String> {
        let bind: SocketAddr = self
            .bind_text
            .trim()
            .parse()
            .map_err(|e| format!("invalid bind address: {e}"))?;
        let token = if self.no_auth { None } else { Some(self.token) };
        let token_file = {
            let trimmed = self.token_file_text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        };
        Ok(TcpScriptConfig {
            bind,
            token,
            token_file,
        })
    }
}

impl SettingsDraft {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            load_last: cfg.load_last,
            tcp_enabled: cfg.script_tcp.is_some(),
            tcp: cfg
                .script_tcp
                .as_ref()
                .map(TcpDraft::from_config)
                .unwrap_or_default(),
            current_path: cfg.current_path.clone(),
        }
    }

    pub fn to_config(&self) -> Result<Config, String> {
        let script_tcp = if self.tcp_enabled {
            Some(self.tcp.to_config()?)
        } else {
            None
        };
        Ok(Config {
            current_path: self.current_path.clone(),
            load_last: self.load_last,
            script_tcp,
        })
    }

    pub fn bind_error(&self) -> Option<String> {
        if !self.tcp_enabled {
            return None;
        }
        self.tcp
            .bind_text
            .trim()
            .parse::<SocketAddr>()
            .err()
            .map(|e| format!("invalid bind address: {e}"))
    }

    pub fn is_valid(&self) -> bool {
        self.to_config().is_ok()
    }

    pub fn regenerate_token(&mut self) {
        self.tcp.token = Uuid::new_v4();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> Config {
        Config {
            current_path: None,
            load_last: false,
            script_tcp: None,
        }
    }

    #[test]
    fn roundtrip_empty() {
        let cfg = empty_config();
        let draft = SettingsDraft::from_config(&cfg);
        let back = draft.to_config().unwrap();
        assert!(!back.load_last);
        assert!(back.script_tcp.is_none());
        assert!(back.current_path.is_none());
    }

    #[test]
    fn roundtrip_load_last() {
        let mut cfg = empty_config();
        cfg.load_last = true;
        cfg.current_path = Some(PathBuf::from("/tmp/g.json"));
        let draft = SettingsDraft::from_config(&cfg);
        let back = draft.to_config().unwrap();
        assert!(back.load_last);
        assert_eq!(back.current_path, Some(PathBuf::from("/tmp/g.json")));
    }

    #[test]
    fn roundtrip_full_tcp() {
        let token = Uuid::new_v4();
        let mut cfg = empty_config();
        cfg.script_tcp = Some(TcpScriptConfig {
            bind: "127.0.0.1:7000".parse().unwrap(),
            token: Some(token),
            token_file: Some(PathBuf::from("/tmp/tok.json")),
        });
        let draft = SettingsDraft::from_config(&cfg);
        let back = draft.to_config().unwrap();
        let tcp = back.script_tcp.unwrap();
        assert_eq!(tcp.bind, "127.0.0.1:7000".parse().unwrap());
        assert_eq!(tcp.token, Some(token));
        assert_eq!(tcp.token_file, Some(PathBuf::from("/tmp/tok.json")));
    }

    #[test]
    fn roundtrip_tcp_no_auth_no_file() {
        let mut cfg = empty_config();
        cfg.script_tcp = Some(TcpScriptConfig {
            bind: "0.0.0.0:9000".parse().unwrap(),
            token: None,
            token_file: None,
        });
        let draft = SettingsDraft::from_config(&cfg);
        assert!(draft.tcp.no_auth);
        let back = draft.to_config().unwrap();
        let tcp = back.script_tcp.unwrap();
        assert!(tcp.token.is_none());
        assert!(tcp.token_file.is_none());
    }

    #[test]
    fn invalid_bind_rejected() {
        let mut draft = SettingsDraft::from_config(&empty_config());
        draft.tcp_enabled = true;
        draft.tcp.bind_text = "not a socket".to_string();
        assert!(draft.to_config().is_err());
        assert!(draft.bind_error().is_some());
        assert!(!draft.is_valid());
    }

    #[test]
    fn invalid_bind_ignored_when_tcp_disabled() {
        let mut draft = SettingsDraft::from_config(&empty_config());
        draft.tcp_enabled = false;
        draft.tcp.bind_text = "garbage".to_string();
        assert!(draft.to_config().is_ok());
        assert!(draft.bind_error().is_none());
    }

    #[test]
    fn toggling_tcp_off_preserves_draft_values() {
        let mut draft = SettingsDraft::from_config(&empty_config());
        draft.tcp_enabled = true;
        draft.tcp.bind_text = "127.0.0.1:1234".to_string();
        let saved_token = draft.tcp.token;

        draft.tcp_enabled = false;
        // off => no script_tcp persisted
        assert!(draft.to_config().unwrap().script_tcp.is_none());
        // but the draft body kept its values
        assert_eq!(draft.tcp.bind_text, "127.0.0.1:1234");
        assert_eq!(draft.tcp.token, saved_token);

        draft.tcp_enabled = true;
        let tcp = draft.to_config().unwrap().script_tcp.unwrap();
        assert_eq!(tcp.bind, "127.0.0.1:1234".parse().unwrap());
    }

    #[test]
    fn regenerate_token_changes_value() {
        let mut draft = SettingsDraft::from_config(&empty_config());
        draft.tcp_enabled = true;
        let before = draft.tcp.token;
        draft.regenerate_token();
        assert_ne!(draft.tcp.token, before);
    }

    #[test]
    fn empty_token_file_text_means_none() {
        let mut draft = SettingsDraft::from_config(&empty_config());
        draft.tcp_enabled = true;
        draft.tcp.token_file_text = "   ".to_string();
        let tcp = draft.to_config().unwrap().script_tcp.unwrap();
        assert!(tcp.token_file.is_none());
    }
}
