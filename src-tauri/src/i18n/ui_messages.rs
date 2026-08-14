use std::collections::BTreeMap;
use std::sync::OnceLock;

const UI_MESSAGE_REGISTRY: &str = include_str!("../../../src/shared/contracts/uiMessages.json");
static UI_MESSAGE_KEYS: OnceLock<Vec<String>> = OnceLock::new();

pub fn ui_message_keys() -> &'static [String] {
    UI_MESSAGE_KEYS.get_or_init(|| {
        serde_json::from_str::<BTreeMap<String, bool>>(UI_MESSAGE_REGISTRY)
            .expect("the shared UI message registry is valid JSON")
            .into_keys()
            .collect()
    })
}
