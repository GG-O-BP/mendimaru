use super::*;
use serde_json::json;

fn baseline() -> Snapshot {
    Snapshot {
        observed_at: Utc::now(),
        managed_vm: Some("a".repeat(64)),
        management_generation: Some("b".repeat(32)),
        container: Some("c".repeat(64)),
        container_running: Some(true),
        compose: Some("d".repeat(64)),
        published_ports: Some(vec![]),
        runtime: Some("e".repeat(64)),
        studio: Some(Studio {
            process_id: 42,
            started_at: Utc::now(),
            running: true,
        }),
        build: Some("f".repeat(64)),
        build_watch_generation: Some(0),
    }
}
fn report() -> Report {
    let s = baseline();
    Report {
        version: 1,
        preparation_id: format!("preparation_{}", "a".repeat(32)),
        baseline: s.clone(),
        latest: s,
        observations: 1,
        events: vec![],
        comparable: true,
        missing: vec![],
        actor: "unknown".into(),
        interval_milliseconds: 1000,
    }
}

#[test]
fn separates_recreation_ports_pid_reuse_and_watch_rebuild_without_attributing_an_actor() {
    let changes: Vec<(Component, Classification, Snapshot)> = {
        let s = baseline();
        let mut container = s.clone();
        container.container = Some("0".repeat(64));
        let mut ports = s.clone();
        ports.published_ports = Some(vec![Port {
            guest: 8080,
            host: 8081,
            protocol: "tcp".into(),
            address_digest: "0".repeat(64),
        }]);
        let mut studio = s.clone();
        studio.studio.as_mut().unwrap().started_at += chrono::Duration::seconds(1);
        let mut build = s.clone();
        build.build_watch_generation = Some(1);
        vec![
            (
                Component::Container,
                Classification::EnvironmentChanged,
                container,
            ),
            (
                Component::PublishedPorts,
                Classification::EnvironmentChanged,
                ports,
            ),
            (Component::Studio, Classification::SessionLost, studio),
            (Component::Build, Classification::BuildChanged, build),
        ]
    };
    for (component, class, next) in changes {
        let mut r = report();
        // Use identical Studio identity except in the PID reuse scenario.
        if component != Component::Studio {
            r.baseline.studio = next.studio.clone();
        }
        r.observe(next.clone(), true, true, true);
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].classification, class);
        assert_eq!(r.events[0].component, component);
        assert!(!r.comparable);
        // A revert never erases the first observation or authorizes a retry.
        for _ in 0..100 {
            r.observe(r.baseline.clone(), true, true, true);
        }
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].observation, next);
        assert_eq!(r.actor, "unknown");
    }
}

#[test]
fn stable_complete_partial_and_unavailable_observations_have_explicit_comparability() {
    let mut r = report();
    r.observe(r.baseline.clone(), true, true, true);
    assert!(r.comparable && r.valid());
    r.missing.push(Component::Build);
    r.observe(r.baseline.clone(), true, true, false);
    assert!(!r.comparable && !r.interrupted() && r.valid());
    let mut next = r.baseline.clone();
    next.runtime = None;
    next.studio = None;
    r.observe(next, true, true, true);
    assert!(r
        .events
        .iter()
        .all(|e| e.classification == Classification::SessionLost));
    let mut r = report();
    r.baseline.container = None;
    r.observe(r.baseline.clone(), true, true, true);
    assert_eq!(
        r.events[0].classification,
        Classification::ObservationUnavailable
    );
    r.actor = "external-controller".into();
    assert!(!r.valid());
}

#[test]
fn inspection_is_allowlisted_bounded_ordered_and_deduplicated() {
    let binding = json!({"HostIp":"127.0.0.1","HostPort":"8080"});
    let raw = json!({"id":"a".repeat(64),"running":true,"ports":{"8080/tcp":[binding.clone(),binding]},"Env":"private-canary"});
    let (_, _, ports) = parse_container(&serde_json::to_vec(&raw).unwrap()).unwrap();
    assert_eq!(ports.len(), 1);
    assert!(!serde_json::to_string(&ports).unwrap().contains("127.0.0.1"));
    let mut invalid = raw.clone();
    invalid["ports"]["8080/tcp"][0]["HostIp"] = json!("private-canary");
    assert!(parse_container(&serde_json::to_vec(&invalid).unwrap()).is_none());
    invalid = raw;
    invalid["id"] = json!("private-canary");
    assert!(parse_container(&serde_json::to_vec(&invalid).unwrap()).is_none());
}

#[test]
fn marker_replacement_identical_content_and_parent_recreation_cannot_reuse_a_generation() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("build");
    std::fs::create_dir(&dir).unwrap();
    let marker = dir.join("generation");
    std::fs::write(&marker, "first").unwrap();
    let watch = BuildWatch::new(&marker).unwrap();
    let before = watch.snapshot();
    assert!(before.0.is_some());
    std::fs::write(dir.join("next"), "first").unwrap();
    std::fs::rename(dir.join("next"), &marker).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while watch.snapshot().1 == before.1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let after = watch.snapshot();
    assert_ne!(before.0, after.0);
    assert_ne!(before.1, after.1);
    // Parent replacement must not silently restart observation in the same run.
    std::fs::rename(&dir, root.path().join("old")).unwrap();
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(&marker, "first").unwrap();
    assert_ne!(watch.snapshot().0, after.0);
    let oversized = root.path().join("large");
    std::fs::write(&oversized, vec![0; MAX_MARKER as usize + 1]).unwrap();
    assert!(BuildWatch::new(&oversized).is_err());
    std::os::unix::fs::symlink(&marker, root.path().join("link")).unwrap();
    assert!(BuildWatch::new(&root.path().join("link")).is_err());
    watch.failed.store(true, Ordering::SeqCst);
    assert_eq!(watch.snapshot(), (None, None));
}
