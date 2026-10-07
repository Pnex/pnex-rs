//! R9 (SEC-W2): a vault secret is bound to its destination.
//!
//! Only owner/admin wire secrets. A writer without `can_manage_secrets`
//! (member, assistant) may keep a vault reference only where the same
//! consumer (same flow — latest or deployed version — or same channel)
//! already holds it, on the same field, toward the same destination.
//! Anything else involving a reference is refused
//! (`secret-destination-locked`): picking a secret, moving one to another
//! field, pointing a field that holds one to another URL / host / port,
//! a duplicated or imported flow carrying references, a test draft that
//! differs from the stored channel. Typed values are already owner/admin
//! only (D117).

use uuid::Uuid;

use super::store::StoreError;

/// `(field, secret id, destination key)` of one vault reference of a
/// consumer. Field = `node_id/auth.token` for a flow, the config field for
/// a channel. Destination = `""` when the kind fixes it.
pub type Binding = (String, Uuid, String);

/// Checks `wanted` against what the consumer already holds. `allowed =
/// None` = owner/admin: no restriction.
pub fn check(allowed: Option<&[Binding]>, wanted: &[Binding]) -> Result<(), StoreError> {
    let Some(allowed) = allowed else {
        return Ok(());
    };
    match wanted.iter().find(|b| !allowed.contains(b)) {
        Some((field, _, _)) => Err(StoreError::DestinationLocked {
            field: field.clone(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(field: &str, id: u128, dest: &str) -> Binding {
        (field.into(), Uuid::from_u128(id), dest.into())
    }

    #[test]
    fn owner_admin_is_not_restricted() {
        assert!(check(None, &[b("n/auth.token", 1, "https://x:443")]).is_ok());
    }

    #[test]
    fn only_an_identical_binding_is_kept() {
        let held = [b("n/auth.token", 1, "https://api.example.com:443")];
        assert!(check(Some(&held), &held).is_ok());
        assert!(
            check(Some(&held), &[]).is_ok(),
            "removing a reference is free"
        );
        for wanted in [
            b("n/auth.token", 1, "https://evil.example.net:443"),
            b("m/auth.token", 1, "https://api.example.com:443"),
            b("n/auth.token", 2, "https://api.example.com:443"),
        ] {
            assert!(matches!(
                check(Some(&held), &[wanted]),
                Err(StoreError::DestinationLocked { .. })
            ));
        }
    }
}
