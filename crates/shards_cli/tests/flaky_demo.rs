//! DEMO ONLY, never merged: a test that fails on its first attempt and passes on a retry
//! (nextest runs a retry in a new process, so the first attempt leaves a marker file).
//! Under profile `ci` (`flaky-result = "fail"`) the run must fail.

#[test]
fn fails_once_then_passes() {
    let marker = std::env::temp_dir().join(format!("shards_flaky_demo_{}", std::env::var("GITHUB_RUN_ID").unwrap_or_default()));
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
        return;
    }
    std::fs::write(&marker, "first attempt").unwrap();
    panic!("planted flaky test: the first attempt fails");
}
