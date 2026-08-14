use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    Ltr,
    Rtl,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleOption {
    pub id: String,
    pub native_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalizationBundle {
    pub locale: String,
    pub preference: String,
    pub direction: TextDirection,
    pub available_locales: Vec<LocaleOption>,
    pub messages: BTreeMap<String, String>,
    pub numbers: Vec<String>,
}
