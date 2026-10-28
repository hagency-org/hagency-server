use super::*;
use serde_json::json;
#[test]
fn canonical_plain_and_encrypted_descriptors_are_bounded_and_private_in_debug() {
    let file = json!({"msgtype":"m.file","body":"caption","filename":"untrusted 中文.bin","url":"mxc://media.test/opaque","info":{"mimetype":"application/octet-stream","size":5}});
    let a = Attachment::from_content(&file).unwrap();
    a.validate().unwrap();
    assert_eq!(a.filename, "untrusted 中文.bin");
    assert_eq!(a.declared_size, Some(5));
    assert!(!format!("{a:?}").contains("mxc://"));
    let mut encrypted = file;
    encrypted.as_object_mut().unwrap().remove("url");
    encrypted["file"] = json!({"url":"mxc://media.test/secret","v":"v2","key":{"kty":"oct","alg":"A256CTR","ext":true,"key_ops":["encrypt","decrypt"],"k":URL_SAFE_NO_PAD.encode([7;32])},"iv":STANDARD_NO_PAD.encode([8;16]),"hashes":{"sha256":STANDARD_NO_PAD.encode([9;32])}});
    let e = Attachment::from_content(&encrypted).unwrap();
    e.validate().unwrap();
    assert_ne!(e.descriptor_digest, a.descriptor_digest);
    assert!(!format!("{e:?}").contains("BwcH"));
    encrypted["file"]["key"]["k"] = "wrong".into();
    assert!(Attachment::from_content(&encrypted).is_err());
    for url in [
        "https://evil.test/x",
        "file:///etc/passwd",
        "mxc://user@host/id",
        "mxc://host/id?secret=x",
        "mxc://host/../x",
        "mxc://host/id#fragment",
    ] {
        assert!(
            Attachment::from_content(&json!({"msgtype":"m.image","body":"image","url":url}))
                .is_err(),
            "{url}"
        );
    }
    let mut changed = a;
    changed.filename = "changed".into();
    assert!(changed.validate().is_err());
}

#[test]
fn portable_wire_commitment_and_captions() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("wire-fixtures.json")).unwrap();
    for fixture in fixtures {
        let a = Attachment::from_content(&fixture["content"]).unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap(), fixture["descriptor"]);
        let decoded: Attachment = serde_json::from_value(fixture["descriptor"].clone()).unwrap();
        decoded.validate().unwrap();
        let mut caption = fixture["content"].clone();
        caption.as_object_mut().unwrap().remove("filename");
        let caption = Attachment::from_content(&caption).unwrap();
        assert!(caption.filename.contains('\n'));
        caption.validate().unwrap();
        for info in [
            json!(null),
            json!("bad"),
            json!({"size":-1}),
            json!({"mimetype":null}),
        ] {
            let mut invalid = fixture["content"].clone();
            invalid["info"] = info;
            assert!(Attachment::from_content(&invalid).is_err());
        }
    }
}
