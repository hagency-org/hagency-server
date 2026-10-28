use super::*;
#[test]
fn file_manifest_commits_exact_metadata_and_content_hash() {
    let f = Manifest {
        filename: "报告.bin".into(),
        caption: Some("Check this\nnow".into()),
        bytes: 5,
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
    };
    let mut changed = f.clone();
    changed.filename = "other.bin".into();
    assert_ne!(f.digest().unwrap(), changed.digest().unwrap());
    for name in ["", "../file", "directory/file", "with\\path", "bad\nname"] {
        let mut f = f.clone();
        f.filename = name.into();
        assert!(f.validate().is_err());
    }
    let mut huge = f.clone();
    huge.bytes = MAX_BYTES as u64 + 1;
    assert!(huge.validate().is_err());
    let body = content(
        &f.filename,
        f.caption.as_deref(),
        f.bytes,
        "mxc://example.test/file",
        "file-id",
        "!room:test",
        "$original:test",
    );
    assert_eq!(body["m.relates_to"]["event_id"], "$original:test");
    assert!(body.get("m.mentions").is_none());
    assert_eq!(body["body"], "Check this\nnow");
}
