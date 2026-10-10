use unoone_android_vault_jni::{derive, REQUIRED_AVAILABLE_BYTES};

#[test]
fn unchanged_shared_kdf_vectors_match_production_vault_core() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../vault-core/test-vectors/vault-cross-platform.json"
    ))
    .unwrap();
    for v in data["kdf"].as_array().unwrap() {
        assert_eq!(v["memory_kib"], 262144);
        assert_eq!(v["iterations"], 3);
        assert_eq!(v["parallelism"], 4);
        assert_eq!(v["output_len"], 32);
        let salt = hex::decode(v["salt_hex"].as_str().unwrap()).unwrap();
        let key = derive(
            v["password_utf8"].as_str().unwrap().as_bytes(),
            &salt,
            REQUIRED_AVAILABLE_BYTES,
            false,
        )
        .unwrap();
        assert_eq!(
            hex::encode(key.as_ref()),
            v["expected_key_hex"].as_str().unwrap()
        );
    }
}
