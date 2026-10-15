//! Local approval of principal-bound credentials. Nothing on the network can
//! invoke approval, manufacture a principal, or read an existing credential.
use crate::TransportError;
use rui_protocol_core::commands::Id;
use rui_protocol_core::session::VerifiedAuthorization;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use zeroize::Zeroize;

pub struct Credential([u8; 32]);
impl Credential {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn expose_for_tls_authentication(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential([REDACTED])")
    }
}
impl Drop for Credential {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPairing {
    pub request_id: Id,
    pub client_instance_id: Id,
}
struct Approved {
    client_instance_id: Id,
    authorization: VerifiedAuthorization,
    lifetime: rui_host_broker::AuthorizationLifetime,
}
impl Drop for Approved {
    fn drop(&mut self) {
        self.lifetime.revoke();
    }
}
/// Bounded authority state owned by the local operator. Approved records hold
/// SHA-256 credential digests; raw bearer credentials are returned once only.
pub struct PairingAuthority {
    capacity: usize,
    pending: BTreeMap<Id, PendingPairing>,
    approved: BTreeMap<[u8; 32], Approved>,
}
impl PairingAuthority {
    /// Trusted local persistence only. Contains credential digests, never bearer secrets.
    pub fn approved_records(&self) -> Vec<([u8; 32], Id, VerifiedAuthorization)> {
        self.approved
            .iter()
            .map(|(digest, a)| (*digest, a.client_instance_id, a.authorization.clone()))
            .collect()
    }
    pub fn restore_local(
        &mut self,
        digest: [u8; 32],
        client_instance_id: Id,
        authorization: VerifiedAuthorization,
    ) -> Result<(), TransportError> {
        if self.approved.len() >= self.capacity
            || client_instance_id == [0; 16]
            || authorization.principal.is_empty()
            || authorization.principal.len() > 1024
            || authorization.permissions.actions.len() > 128
            || authorization
                .permissions
                .actions
                .iter()
                .any(|a| a.len() > 256)
            || self.approved.contains_key(&digest)
            || self
                .approved
                .values()
                .any(|a| a.client_instance_id == client_instance_id)
        {
            return Err(TransportError::Authentication);
        }
        self.approved.insert(
            digest,
            Approved {
                client_instance_id,
                authorization,
                lifetime: Default::default(),
            },
        );
        Ok(())
    }
    pub fn new(capacity: usize) -> Result<Self, TransportError> {
        if capacity == 0 {
            return Err(TransportError::Limit);
        }
        Ok(Self {
            capacity,
            pending: BTreeMap::new(),
            approved: BTreeMap::new(),
        })
    }
    pub fn request_local_pairing(
        &mut self,
        client_instance_id: Id,
    ) -> Result<PendingPairing, TransportError> {
        if client_instance_id == [0; 16]
            || self.pending.len() + self.approved.len() >= self.capacity
        {
            return Err(TransportError::Limit);
        }
        let mut request_id = [0; 16];
        getrandom::getrandom(&mut request_id).map_err(|_| TransportError::Entropy)?;
        let request = PendingPairing {
            request_id,
            client_instance_id,
        };
        self.pending.insert(request_id, request.clone());
        Ok(request)
    }
    /// Explicit local approval, after the operator reviews the request and its
    /// requested permissions. This method is never a remote message handler.
    pub fn approve_local(
        &mut self,
        request_id: Id,
        authorization: VerifiedAuthorization,
    ) -> Result<Credential, TransportError> {
        if authorization.principal.is_empty()
            || authorization.principal.len() > 1024
            || authorization.permissions.actions.len() > 128
            || authorization
                .permissions
                .actions
                .iter()
                .any(|a| a.len() > 256)
        {
            return Err(TransportError::Authentication);
        }
        let request = self
            .pending
            .get(&request_id)
            .ok_or(TransportError::Authentication)?;
        if self
            .approved
            .values()
            .any(|a| a.client_instance_id == request.client_instance_id)
        {
            return Err(TransportError::Authentication);
        }
        let mut credential = [0; 32];
        getrandom::getrandom(&mut credential).map_err(|_| TransportError::Entropy)?;
        let digest = Sha256::digest(credential).into();
        self.approved.insert(
            digest,
            Approved {
                client_instance_id: request.client_instance_id,
                authorization,
                lifetime: rui_host_broker::AuthorizationLifetime::new(),
            },
        );
        self.pending.remove(&request_id);
        Ok(Credential(credential))
    }
    pub fn deny_local(&mut self, request_id: Id) -> bool {
        self.pending.remove(&request_id).is_some()
    }
    pub fn revoke_local(&mut self, client_instance_id: Id) -> bool {
        let old = self.approved.len();
        self.approved
            .retain(|_, a| a.client_instance_id != client_instance_id);
        old != self.approved.len()
    }
    pub fn authenticated(
        &self,
        raw: &[u8],
        client_instance_id: Id,
    ) -> Result<VerifiedAuthorization, TransportError> {
        if raw.len() != 32 {
            return Err(TransportError::Authentication);
        }
        let digest: [u8; 32] = Sha256::digest(raw).into();
        let approved = self
            .approved
            .get(&digest)
            .ok_or(TransportError::Authentication)?;
        if approved.client_instance_id != client_instance_id {
            return Err(TransportError::Authentication);
        }
        Ok(approved.authorization.clone())
    }
    pub(crate) fn approval_lifetime(
        &self,
        raw: &[u8],
        client: Id,
    ) -> Result<rui_host_broker::AuthorizationLifetime, TransportError> {
        let digest = self.approval_id(raw, client)?;
        Ok(self
            .approved
            .get(&digest)
            .ok_or(TransportError::Authentication)?
            .lifetime
            .clone())
    }
    pub(crate) fn approval_id(&self, raw: &[u8], client: Id) -> Result<[u8; 32], TransportError> {
        self.authenticated(raw, client)?;
        Ok(Sha256::digest(raw).into())
    }
    pub(crate) fn remains_authorized(
        &self,
        grant: [u8; 32],
        client: Id,
        authorization: &VerifiedAuthorization,
    ) -> bool {
        self.approved.get(&grant).is_some_and(|a| {
            a.client_instance_id == client
                && a.authorization.principal == authorization.principal
                && a.authorization.policy_version == authorization.policy_version
                && a.authorization.permissions == authorization.permissions
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::session::Permissions;
    #[test]
    fn persisted_digests_restore_bound_credentials_and_revoke_lifetimes() {
        let mut source = PairingAuthority::new(2).unwrap();
        let request = source.request_local_pairing([3; 16]).unwrap();
        let auth = VerifiedAuthorization {
            principal: "device".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["Activate".into()].into_iter().collect(),
            },
        };
        let credential = source
            .approve_local(request.request_id, auth.clone())
            .unwrap();
        let (digest, client, saved) = source.approved_records().pop().unwrap();
        drop(source);
        let mut restored = PairingAuthority::new(2).unwrap();
        restored.restore_local(digest, client, saved).unwrap();
        assert_eq!(
            restored
                .authenticated(credential.expose_for_tls_authentication(), client)
                .unwrap()
                .principal,
            auth.principal
        );
        assert!(restored
            .authenticated(credential.expose_for_tls_authentication(), [4; 16])
            .is_err());
        assert!(restored.restore_local(digest, [4; 16], auth).is_err());
        let lifetime = restored
            .approval_lifetime(credential.expose_for_tls_authentication(), client)
            .unwrap();
        restored.revoke_local(client);
        assert!(!lifetime.is_authorized());
        assert!(restored.approved_records().is_empty());
        assert!(restored
            .authenticated(credential.expose_for_tls_authentication(), client)
            .is_err());
    }
    #[test]
    fn revoked_grant_cannot_survive_reapproval_with_same_principal() {
        let mut pairing = PairingAuthority::new(2).unwrap();
        let auth = VerifiedAuthorization {
            principal: "operator".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["Activate".into()].into_iter().collect(),
            },
        };
        let request = pairing.request_local_pairing([3; 16]).unwrap();
        let credential = pairing
            .approve_local(request.request_id, auth.clone())
            .unwrap();
        let grant = pairing
            .approval_id(credential.expose_for_tls_authentication(), [3; 16])
            .unwrap();
        assert!(pairing.remains_authorized(grant, [3; 16], &auth));
        assert!(pairing.revoke_local([3; 16]));
        let mut narrower = auth.clone();
        narrower.permissions.can_control = false;
        narrower.permissions.actions.clear();
        let request = pairing.request_local_pairing([3; 16]).unwrap();
        let replacement = pairing
            .approve_local(request.request_id, narrower.clone())
            .unwrap();
        assert!(!pairing.remains_authorized(grant, [3; 16], &auth));
        assert_eq!(
            pairing
                .authenticated(credential.expose_for_tls_authentication(), [3; 16])
                .unwrap_err(),
            TransportError::Authentication
        );
        assert_eq!(
            pairing
                .authenticated(replacement.expose_for_tls_authentication(), [4; 16])
                .unwrap_err(),
            TransportError::Authentication
        );
        assert!(
            !pairing
                .authenticated(replacement.expose_for_tls_authentication(), [3; 16])
                .unwrap()
                .permissions
                .can_control
        );
        assert_eq!(
            pairing.request_local_pairing([0; 16]).unwrap_err(),
            TransportError::Limit
        );
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    use rui_protocol_core::session::Permissions;
    #[test]
    fn immutable_approval_lifetime_ends_on_local_revoke_reapproval_and_authority_drop() {
        let mut authority = PairingAuthority::new(1).unwrap();
        let request = authority.request_local_pairing([3; 16]).unwrap();
        let auth = VerifiedAuthorization {
            principal: "alice".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: false,
                actions: Default::default(),
            },
        };
        let credential = authority
            .approve_local(request.request_id, auth.clone())
            .unwrap();
        let old = authority
            .approval_lifetime(credential.expose_for_tls_authentication(), [3; 16])
            .unwrap();
        assert!(old.is_authorized());
        assert!(authority.revoke_local([3; 16]));
        assert!(!old.is_authorized());
        let request = authority.request_local_pairing([3; 16]).unwrap();
        let replacement = authority.approve_local(request.request_id, auth).unwrap();
        let fresh = authority
            .approval_lifetime(replacement.expose_for_tls_authentication(), [3; 16])
            .unwrap();
        assert!(!old.is_authorized());
        assert!(fresh.is_authorized());
        drop(authority);
        assert!(!fresh.is_authorized());
    }
}
