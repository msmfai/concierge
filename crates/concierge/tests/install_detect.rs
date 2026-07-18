//! Owned-DLC detection: the rendered load order must carry only the DLC a
//! player actually has in their install's Data folder — not the adapter's
//! assume-everything default (some players are missing some DLC).
#![allow(clippy::unwrap_used, clippy::panic)]

#[test]
fn owned_base_plugins_keeps_base_and_present_dlc_only() {
    concierge_games::register();
    let dir = std::env::temp_dir().join(format!("cg-owned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let data = dir.join("Data");
    std::fs::create_dir_all(&data).unwrap();
    // This install owns the base game + Far Harbor only.
    std::fs::write(data.join("Fallout4.esm"), "").unwrap();
    std::fs::write(data.join("DLCCoast.esm"), "").unwrap();

    let owned = concierge::install::owned_base_plugins("fallout4", &dir).unwrap();
    assert!(
        owned.contains(&"Fallout4.esm".to_owned()),
        "base master always present: {owned:?}"
    );
    assert!(
        owned.contains(&"DLCCoast.esm".to_owned()),
        "owned DLC kept: {owned:?}"
    );
    assert!(
        !owned.contains(&"DLCNukaWorld.esm".to_owned()),
        "un-owned DLC dropped: {owned:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
