//! SQLite persistence for Wave 5A hardware-signed SLSA L3 / In-Toto provenance records.

use anyhow::Result;
use neutrino_proto::{ProvenanceKind, ProvenanceRecord};
use rusqlite::params;

use crate::Store;

fn kind_str(kind: ProvenanceKind) -> &'static str {
    match kind {
        ProvenanceKind::Merge => "merge",
        ProvenanceKind::Ci => "ci",
        ProvenanceKind::Deploy => "deploy",
    }
}

fn parse_kind(s: &str) -> ProvenanceKind {
    match s {
        "merge" => ProvenanceKind::Merge,
        "deploy" => ProvenanceKind::Deploy,
        _ => ProvenanceKind::Ci,
    }
}

impl Store {
    pub fn add_provenance(&self, rec: &ProvenanceRecord) -> Result<i64> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO provenance (
                repo, sha, kind, statement_json, server_key_id, server_public_key, server_signature,
                device_id, device_public_key, device_signature, device_signed_at, device_nonce,
                device_payload, git_ref, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                rec.repo,
                rec.sha,
                kind_str(rec.kind),
                rec.statement_json,
                rec.server_key_id,
                rec.server_public_key,
                rec.server_signature,
                rec.device_id,
                rec.device_public_key,
                rec.device_signature,
                rec.device_signed_at,
                rec.device_nonce,
                rec.device_payload,
                rec.git_ref,
                rec.created_at,
            ],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn update_provenance_git_ref(&self, id: i64, git_ref: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("UPDATE provenance SET git_ref = ?1 WHERE id = ?2", params![git_ref, id])?;
        Ok(())
    }

    pub fn provenance_for_sha(&self, repo: &str, sha: &str) -> Result<Vec<ProvenanceRecord>> {
        let db = self.db.lock().unwrap();
        let rows = db
            .prepare(
                "SELECT id, repo, sha, kind, statement_json, server_key_id, server_public_key, server_signature,
                        device_id, device_public_key, device_signature, device_signed_at, device_nonce,
                        device_payload, git_ref, created_at
                 FROM provenance WHERE repo = ?1 AND sha = ?2 ORDER BY id",
            )?
            .query_map(params![repo, sha], |r| {
                Ok(ProvenanceRecord {
                    id: r.get(0)?,
                    repo: r.get(1)?,
                    sha: r.get(2)?,
                    kind: parse_kind(&r.get::<_, String>(3)?),
                    statement_json: r.get(4)?,
                    server_key_id: r.get(5)?,
                    server_public_key: r.get(6)?,
                    server_signature: r.get(7)?,
                    device_id: r.get(8)?,
                    device_public_key: r.get(9)?,
                    device_signature: r.get(10)?,
                    device_signed_at: r.get(11)?,
                    device_nonce: r.get(12)?,
                    device_payload: r.get(13)?,
                    git_ref: r.get(14)?,
                    created_at: r.get(15)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_round_trip() {
        let s = Store::in_memory().unwrap();
        let rec = ProvenanceRecord {
            id: 0,
            repo: "app".into(),
            sha: "deadbeef".into(),
            kind: ProvenanceKind::Merge,
            statement_json: "{}".into(),
            server_key_id: "srv1".into(),
            server_public_key: "pub1".into(),
            server_signature: "sig1".into(),
            device_id: Some("phone1".into()),
            device_public_key: Some("dpub1".into()),
            device_signature: Some("dsig1".into()),
            device_signed_at: Some(1234),
            device_nonce: Some("nonce".into()),
            device_payload: Some("payload".into()),
            git_ref: None,
            created_at: 1235,
        };
        let id = s.add_provenance(&rec).unwrap();
        s.update_provenance_git_ref(id, "refs/neutrino/attestations/deadbeef").unwrap();
        let list = s.provenance_for_sha("app", "deadbeef").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].git_ref.as_deref(), Some("refs/neutrino/attestations/deadbeef"));
    }
}
