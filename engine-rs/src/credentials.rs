use std::path::PathBuf;

// Credential store: secrets live in the OS keyring, never on disk in
// plaintext and never in any puppetty log. Only a *ref* (name) is stored,
// logged, or shown to a decider/agent — the secret is fetched at the last
// moment and written straight into the PTY. The keyring can't enumerate our
// entries, so a names-only registry file is kept alongside.

const SERVICE: &str = "puppetty";

fn registry_path() -> PathBuf {
    let dir = dirs::home_dir().unwrap_or_default().join(".puppetty");
    std::fs::create_dir_all(&dir).ok();
    dir.join("credentials.json")
}

fn read_registry() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(registry_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| serde_json::from_value(v["refs"].clone()).ok())
        .unwrap_or_default()
}

fn write_registry(mut refs: Vec<String>) {
    refs.sort();
    refs.dedup();
    let _ = std::fs::write(
        registry_path(),
        serde_json::to_string_pretty(&serde_json::json!({ "refs": refs })).unwrap(),
    );
}

/// Registry refs whose secret still exists in the keyring (reconciled).
pub fn list_refs() -> Vec<String> {
    let all = read_registry();
    let refs: Vec<String> = all
        .iter()
        .filter(|r| get_credential(r).is_some())
        .cloned()
        .collect();
    if refs.len() != all.len() {
        write_registry(refs.clone());
    }
    refs
}

pub fn set_credential(cred_ref: &str, secret: &str) -> Result<(), String> {
    let ok = !cred_ref.is_empty()
        && cred_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if !ok {
        return Err("ref must match [A-Za-z0-9_.-]+".into());
    }
    keyring::Entry::new(SERVICE, cred_ref)
        .and_then(|e| e.set_password(secret))
        .map_err(|e| e.to_string())?;
    let mut refs = read_registry();
    refs.push(cred_ref.to_string());
    write_registry(refs);
    Ok(())
}

/// The secret, or None if unknown. Callers MUST NOT log the result and
/// should write it directly to the PTY.
pub fn get_credential(cred_ref: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, cred_ref)
        .ok()?
        .get_password()
        .ok()
}

pub fn delete_credential(cred_ref: &str) -> bool {
    let existed = keyring::Entry::new(SERVICE, cred_ref)
        .and_then(|e| e.delete_credential())
        .is_ok();
    write_registry(
        read_registry()
            .into_iter()
            .filter(|r| r != cred_ref)
            .collect(),
    );
    existed
}
