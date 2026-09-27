//! Optional attribution recorded for owner review, never deletion authority.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provenance {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub project: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub actor: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub session: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub host: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub creation_method: Option<String>,
}

pub fn checked(value: Option<String>, field: &str) -> Result<Option<String>, String> {
  match value {
    Some(value) => clean(&value)
      .map(Some)
      .ok_or_else(|| format!("{field} must be nonempty and contain no control characters")),
    None => Ok(None),
  }
}

pub fn clean(value: &str) -> Option<String> {
  let value = value.trim();
  if value.is_empty() || value.chars().any(char::is_control) {
    None
  } else {
    Some(value.to_string())
  }
}

pub fn from_env(name: &str) -> Option<String> {
  std::env::var(name).ok().and_then(|value| clean(&value))
}
