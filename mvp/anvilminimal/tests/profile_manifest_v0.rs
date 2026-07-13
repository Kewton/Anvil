use anvilminimal::planner::profile_manifest::{
    MANIFEST_V0_SECTIONS, ManifestStatus, nextjs_manifest,
};

#[test]
fn embedded_nextjs_manifest_loads_and_resolves_every_check() {
    let manifest = nextjs_manifest();

    assert_eq!(manifest.metadata.id, "nextjs");
    assert_eq!(manifest.metadata.display_name, "Next.js");
    assert_eq!(manifest.metadata.schema_version.as_str(), "v0");
    assert_eq!(manifest.metadata.status, ManifestStatus::Draft);
    assert_eq!(
        manifest
            .plan
            .phases
            .iter()
            .map(|phase| phase.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "project-setup",
            "core-implementation",
            "contract-wiring",
            "build-verification",
        ]
    );

    let resolved = manifest.resolve().expect("all check bindings must resolve");
    assert_eq!(resolved.len(), 3);
    assert_eq!(resolved.values().map(Vec::len).sum::<usize>(), 7);
}

#[test]
fn manifest_v0_root_sections_match_golden() {
    let actual = format!("{}\n", MANIFEST_V0_SECTIONS.join("\n"));
    assert_eq!(
        actual,
        include_str!("golden/profile_manifest_v0_sections.txt")
    );
}
