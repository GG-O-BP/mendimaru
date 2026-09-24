//! Read-only, bounded run observations. This is evidence, never lifecycle authority.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Classification {
    EnvironmentChanged,
    SessionLost,
    BuildChanged,
    ObservationUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Component {
    ManagedVm,
    Container,
    Compose,
    PublishedPorts,
    Runtime,
    Studio,
    Build,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Port {
    pub guest: u16,
    pub protocol: String,
    pub host: u16,
    pub address_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Studio {
    pub process_id: u32,
    pub started_at: DateTime<Utc>,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub observed_at: DateTime<Utc>,
    pub managed_vm: Option<String>,
    pub management_generation: Option<String>,
    pub container: Option<String>,
    pub container_running: Option<bool>,
    pub compose: Option<String>,
    pub published_ports: Option<Vec<Port>>,
    pub runtime: Option<String>,
    pub studio: Option<Studio>,
    pub build: Option<String>,
    pub build_watch_generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub classification: Classification,
    pub component: Component,
    pub observation: Snapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub version: u32,
    pub preparation_id: String,
    pub baseline: Snapshot,
    pub latest: Snapshot,
    pub observations: u64,
    pub events: Vec<Event>,
    pub comparable: bool,
    pub missing: Vec<Component>,
    pub actor: String,
    pub interval_milliseconds: u32,
}

impl Report {
    pub(crate) fn interrupted(&self) -> bool {
        !self.events.is_empty()
    }

    pub(crate) fn valid(&self) -> bool {
        fn digest(v: &Option<String>) -> bool {
            v.as_ref().is_none_or(|s| hex(s, 64))
        }
        fn hex(s: &str, n: usize) -> bool {
            s.len() == n
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        let snapshots = std::iter::once(&self.baseline)
            .chain(std::iter::once(&self.latest))
            .chain(self.events.iter().map(|e| &e.observation));
        self.version == 1
            && self.actor == "unknown"
            && self.interval_milliseconds == 1000
            && self
                .preparation_id
                .strip_prefix("preparation_")
                .is_some_and(|s| hex(s, 32))
            && self.observations > 0
            && self.events.len() <= 7
            && self.missing.len() <= 7
            && self.comparable == (self.events.is_empty() && self.missing.is_empty())
            && self.latest.observed_at >= self.baseline.observed_at
            && snapshots.into_iter().all(|s| {
                digest(&s.managed_vm)
                    && digest(&s.container)
                    && digest(&s.compose)
                    && digest(&s.runtime)
                    && digest(&s.build)
                    && s.management_generation
                        .as_ref()
                        .is_none_or(|g| g == "uninitialized" || hex(g, 32))
                    && s.published_ports.as_ref().is_none_or(|ports| {
                        ports.len() <= 256
                            && ports.iter().all(|p| {
                                p.guest > 0
                                    && p.host > 0
                                    && matches!(p.protocol.as_str(), "tcp" | "udp" | "sctp")
                                    && hex(&p.address_digest, 64)
                            })
                    })
            })
    }

    #[cfg(target_os = "linux")]
    fn observe(&mut self, next: Snapshot, runtime: bool, studio: bool, build: bool) {
        use Classification::*;
        use Component::*;
        // Keep the first evidence per component, even after a change is reverted.
        // Seven components bound the report for arbitrarily long runs.
        let comparisons = [
            (
                ManagedVm,
                self.baseline.managed_vm != next.managed_vm
                    || self.baseline.management_generation != next.management_generation,
                next.managed_vm.is_none() || next.management_generation.is_none(),
                EnvironmentChanged,
            ),
            (
                Container,
                self.baseline.container != next.container
                    || self.baseline.container_running != next.container_running,
                next.container.is_none(),
                EnvironmentChanged,
            ),
            (
                Compose,
                self.baseline.compose != next.compose,
                next.compose.is_none(),
                EnvironmentChanged,
            ),
            (
                PublishedPorts,
                self.baseline.published_ports != next.published_ports,
                next.published_ports.is_none(),
                EnvironmentChanged,
            ),
            (
                Runtime,
                runtime && self.baseline.runtime != next.runtime,
                runtime && next.runtime.is_none(),
                SessionLost,
            ),
            (
                Studio,
                studio && self.baseline.studio != next.studio,
                studio && next.studio.is_none(),
                SessionLost,
            ),
            (
                Build,
                build
                    && (self.baseline.build != next.build
                        || self.baseline.build_watch_generation != next.build_watch_generation),
                build && next.build.is_none(),
                BuildChanged,
            ),
        ];
        for (component, changed, unavailable, classification) in comparisons {
            let stopped = match component {
                Container => next.container_running == Some(false),
                Studio => studio && next.studio.as_ref().is_some_and(|s| !s.running),
                _ => false,
            };
            if (changed || unavailable || stopped)
                && !self.events.iter().any(|e| e.component == component)
            {
                self.events.push(Event {
                    classification: if changed || stopped {
                        classification
                    } else {
                        ObservationUnavailable
                    },
                    component,
                    observation: next.clone(),
                });
            }
        }
        self.latest = next;
        self.observations = self.observations.saturating_add(1);
        self.comparable = self.events.is_empty() && self.missing.is_empty();
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{start, start_prepared};

#[cfg(not(target_os = "linux"))]
pub(crate) struct Observer;
#[cfg(not(target_os = "linux"))]
impl Observer {
    pub(crate) fn url(&self) -> &str {
        ""
    }
}
#[cfg(not(target_os = "linux"))]
pub(crate) async fn start(
    _config: Option<&crate::models::AppConfig>,
    _runtime: Option<&str>,
    marker: Option<&str>,
) -> Result<Option<Observer>, crate::contracts::BackendError> {
    if marker.is_some() {
        return Err(crate::contracts::BackendError::invalid_request(
            "--build-marker requires a WinBoat target",
        ));
    }
    Ok(None)
}
