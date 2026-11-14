//! Tagged API keys (SECURITY_REVIEW §9.1, SEC-31), operator revocation and the bootstrap admin
//! key (T-AUTH-8). A key is `bbk_` + base64url(24 random bytes ‖ first [`KEY_TAG_LEN`] bytes of
//! HMAC-SHA256(pepper, those 24 bytes)): 43 characters, so the stored layout of
//! [`Database::mint_key`] is unchanged, and a forged key fails [`key_tag_valid`] without any
//! database access.
use crate::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::TryRngCore;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Bytes of the truncated HMAC tag inside a key.
pub const KEY_TAG_LEN: usize = 8;
const KEY_RANDOM_LEN: usize = 32 - KEY_TAG_LEN;

fn tag_mac(pepper: &[u8], random: &[u8]) -> Option<Hmac<Sha256>> {
    if pepper.is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(pepper).ok()?;
    mac.update(random);
    Some(mac)
}

/// Shape, decoding and constant-time tag check of a presented key. No database access; an empty
/// pepper accepts nothing.
pub fn key_tag_valid(plaintext: &str, pepper: &[u8]) -> bool {
    let Some(body) = plaintext.strip_prefix("bbk_") else {
        return false;
    };
    if body.len() != 43
        || !body
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return false;
    }
    let Ok(raw) = URL_SAFE_NO_PAD.decode(body) else {
        return false;
    };
    if raw.len() != 32 {
        return false;
    }
    let (random, tag) = raw.split_at(KEY_RANDOM_LEN);
    tag_mac(pepper, random).is_some_and(|mac| mac.verify_truncated_left(tag).is_ok())
}

/// The stored digest of a key: sha256 of its UTF-8 plaintext.
pub fn key_sha256(plaintext: &str) -> [u8; 32] {
    Sha256::digest(plaintext.as_bytes()).into()
}

/// Outcome of [`Database::ensure_bootstrap_admin_key`].
#[derive(Debug, Clone)]
pub enum BootstrapOutcome {
    /// An active `admin` key exists in some workspace; nothing was created.
    ActiveAdminExists,
    /// The bootstrap key was inserted.
    Created(ApiKey),
    /// A row with this digest exists but is revoked or expired; it is never re-activated.
    Inactive(ApiKey),
}

fn valid_label(label: &str) -> Result<()> {
    let chars = label.chars().count();
    if !(1..=100).contains(&chars) || label.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "label must have 1..=100 characters and no control characters".into(),
        ));
    }
    Ok(())
}

impl Database {
    /// Mints a tagged key (SEC-31) and writes the `key.mint` audit row in the same transaction.
    pub async fn mint_key_tagged(
        &self,
        workspace: Uuid,
        label: &str,
        scopes: &[Scope],
        expires_at: Option<DateTime<Utc>>,
        pepper: &[u8],
    ) -> Result<MintedKey> {
        if pepper.is_empty() {
            return Err(Error::Invalid("API key pepper must not be empty".into()));
        }
        if scopes.is_empty() {
            return Err(Error::Invalid("at least one scope is required".into()));
        }
        valid_label(label)?;
        let mut raw = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut raw[..KEY_RANDOM_LEN])
            .map_err(|_| Error::Invalid("operating system entropy unavailable".into()))?;
        let mac = tag_mac(pepper, &raw[..KEY_RANDOM_LEN])
            .ok_or_else(|| Error::Invalid("API key pepper must not be empty".into()))?;
        raw[KEY_RANDOM_LEN..].copy_from_slice(&mac.finalize().into_bytes()[..KEY_TAG_LEN]);
        let plaintext = format!("bbk_{}", URL_SAFE_NO_PAD.encode(raw));
        let scopes: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let mut tx = self.pool.begin().await?;
        let key: ApiKey = sqlx::query_as("INSERT INTO bb.api_keys (workspace_id,key_sha256,display_prefix,label,scopes,expires_at) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *")
            .bind(workspace).bind(key_sha256(&plaintext).as_slice()).bind(&plaintext[..8]).bind(label).bind(&scopes).bind(expires_at).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO bb.audit_events (workspace_id,action,outcome,target_type,target_id,detail) VALUES ($1,'key.mint','ok','api_key',$2,$3)")
            .bind(workspace).bind(key.id.to_string()).bind(json!({"scopes":scopes,"display_prefix":key.display_prefix,"expires_at":expires_at})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(MintedKey { plaintext, key })
    }

    /// Revokes a key of any workspace (operator CLI, `POST /system/api-keys/{id}/revoke`).
    /// Idempotent: an already revoked key keeps its first `revoked_at`. Every call writes a
    /// `key.revoke` audit row (in the revoked key's workspace, with the caller's key and
    /// request id) in the same transaction.
    pub async fn revoke_key_any(&self, id: Uuid, audit: &Audit) -> Result<ApiKey> {
        let mut tx = self.pool.begin().await?;
        let current: ApiKey = sqlx::query_as("SELECT * FROM bb.api_keys WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let already = current.revoked_at.is_some();
        let key = if already {
            current
        } else {
            sqlx::query_as("UPDATE bb.api_keys SET revoked_at=now() WHERE id=$1 RETURNING *")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?
        };
        Audit {
            workspace: Some(key.workspace_id),
            ..audit.clone()
        }
        .write(
            &mut tx,
            "key.revoke",
            "api_key",
            key.id,
            json!({"already_revoked": already}),
        )
        .await?;
        tx.commit().await?;
        Ok(key)
    }

    /// Deployment bootstrap (T-AUTH-8): creates an `admin` key with this digest in `workspace`,
    /// expiring after `ttl`, only while no active `admin` key exists anywhere. A revoked or
    /// expired row with the digest is reported, never re-activated.
    pub async fn ensure_bootstrap_admin_key(
        &self,
        workspace: Uuid,
        digest: &[u8; 32],
        ttl: std::time::Duration,
    ) -> Result<BootstrapOutcome> {
        if ttl.is_zero() {
            return Err(Error::Invalid("bootstrap key TTL must be positive".into()));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM bb.workspaces WHERE id=$1 FOR UPDATE")
            .bind(workspace)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let active: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bb.api_keys WHERE 'admin'=ANY(scopes) AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now()))")
            .fetch_one(&mut *tx).await?;
        if active {
            return Ok(BootstrapOutcome::ActiveAdminExists);
        }
        let existing: Option<ApiKey> =
            sqlx::query_as("SELECT * FROM bb.api_keys WHERE key_sha256=$1")
                .bind(digest.as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(key) = existing {
            return Ok(BootstrapOutcome::Inactive(key));
        }
        let key: ApiKey = sqlx::query_as("INSERT INTO bb.api_keys (workspace_id,key_sha256,display_prefix,label,scopes,expires_at) VALUES ($1,$2,'bbk_boot','bootstrap',ARRAY['admin'],now()+make_interval(secs=>$3::double precision)) RETURNING *")
            .bind(workspace).bind(digest.as_slice()).bind(ttl.as_secs_f64()).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO bb.audit_events (workspace_id,action,outcome,target_type,target_id,detail) VALUES ($1,'key.bootstrap','ok','api_key',$2,$3)")
            .bind(workspace).bind(key.id.to_string()).bind(json!({"expires_at":key.expires_at})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(BootstrapOutcome::Created(key))
    }
}
