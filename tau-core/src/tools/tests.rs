use super::*;

#[test]
fn default_limits_match_m4_contract() {
    let limits = ToolLimits::default();
    assert_eq!(limits.read_lines, 2_000);
    assert_eq!(limits.read_bytes, 50 * 1024);
    assert_eq!(limits.max_line_chars, 2_000);
    assert_eq!(limits.search_matches, 100);
    assert_eq!(limits.glob_results, 100);
    assert_eq!(limits.directory_entries, 2_000);
}
