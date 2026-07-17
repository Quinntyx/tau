use super::Db;
use super::domain::*;
use crate::permissions::{Decision, PermissionEngine, Rule};
use anyhow::{Result, bail};
use uuid::Uuid;

impl Db {
    pub fn save_policy_decision(
        &self,
        session_id: Option<&str>,
        scope: &str,
        actor: &str,
        pattern: &str,
        decision_json: &str,
    ) -> Result<PolicyDecisionRecord> {
        if !matches!(scope, "session" | "global") {
            bail!("unsupported policy decision scope: {scope}");
        }
        if scope == "session" && session_id.is_none() {
            bail!("session policy decisions require a session");
        }
        let id = Uuid::new_v4().to_string();
        let created_at = now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO policy_decisions(id,session_id,scope,actor,pattern,decision_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO UPDATE SET actor=excluded.actor,decision_json=excluded.decision_json,created_at=excluded.created_at",
            rusqlite::params![id, session_id, scope, actor, pattern, decision_json, created_at],
        )?;
        Ok(conn.query_row(
            "SELECT id,session_id,scope,actor,pattern,decision_json,created_at FROM policy_decisions WHERE scope=?1 AND pattern=?2 AND (scope='global' OR session_id=?3)",
            rusqlite::params![scope, pattern, session_id],
            |row| {
                Ok(PolicyDecisionRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    scope: row.get(2)?,
                    actor: row.get(3)?,
                    pattern: row.get(4)?,
                    decision_json: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )?)
    }

    pub fn list_policy_decisions(
        &self,
        session_id: Option<&str>,
        scope: &str,
    ) -> Result<Vec<PolicyDecisionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,session_id,scope,actor,pattern,decision_json,created_at FROM policy_decisions WHERE scope=?1 AND (scope='global' OR session_id=?2) ORDER BY created_at,id",
        )?;
        let rows = stmt.query_map(rusqlite::params![scope, session_id], |row| {
            Ok(PolicyDecisionRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                scope: row.get(2)?,
                actor: row.get(3)?,
                pattern: row.get(4)?,
                decision_json: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Persist the canonical ordered session rules. Replacing the full set in
    /// one transaction prevents a partially updated policy from authorizing a
    /// call after a client reconnects.
    pub fn save_session_permissions(
        &self,
        session_id: &str,
        engine: &PermissionEngine,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM session_permissions WHERE session_id=?1",
            [session_id],
        )?;
        for (ordinal, rule) in engine.rules().iter().enumerate() {
            tx.execute("INSERT INTO session_permissions(session_id,ordinal,pattern,decision) VALUES(?1,?2,?3,?4)", rusqlite::params![session_id, ordinal as i64, rule.pattern, serde_json::to_string(&rule.decision)?])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_session_permissions(&self, session_id: &str) -> Result<PermissionEngine> {
        let conn = self.conn.lock().unwrap();
        let mut engine = PermissionEngine::default();
        let mut stmt = conn.prepare(
            "SELECT pattern,decision FROM session_permissions WHERE session_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pattern, decision) = row?;
            engine.add_rule(Rule::new(
                pattern,
                serde_json::from_str::<Decision>(&decision)?,
            )?);
        }
        Ok(engine)
    }
    pub fn create_plan_revision(
        &self,
        session_id: &str,
        payload: &str,
        actor: &str,
    ) -> Result<PlanRevision> {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO plans(id,session_id,title,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",
            rusqlite::params![&id, session_id, "plan", now],
        )?;
        conn.execute("INSERT INTO plan_revisions(plan_id,revision,payload,actor,created_at) VALUES(?1,1,?2,?3,?4)", rusqlite::params![&id,payload,actor,now])?;
        Ok(PlanRevision {
            plan_id: id,
            revision: 1,
            parent_revision: None,
            payload: payload.into(),
            airtight: false,
            revoked: false,
            actor: actor.into(),
            created_at: now,
        })
    }
    pub fn append_plan_revision(
        &self,
        plan_id: &str,
        expected_revision: i64,
        payload: &str,
        actor: &str,
    ) -> Result<PlanRevision> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let current: i64 = tx.query_row(
            "SELECT active_revision FROM plans WHERE id=?1 AND revoked=0",
            [plan_id],
            |r| r.get(0),
        )?;
        if current != expected_revision {
            bail!("plan revision conflict: expected {expected_revision}, current {current}");
        }
        let revision = current + 1;
        let now = now_ms();
        tx.execute("INSERT INTO plan_revisions(plan_id,revision,parent_revision,payload,actor,created_at) VALUES(?1,?2,?3,?4,?5,?6)", rusqlite::params![plan_id,revision,current,payload,actor,now])?;
        tx.execute(
            "UPDATE plans SET active_revision=?2,updated_at=?3 WHERE id=?1",
            rusqlite::params![plan_id, revision, now],
        )?;
        tx.commit()?;
        Ok(PlanRevision {
            plan_id: plan_id.into(),
            revision,
            parent_revision: Some(current),
            payload: payload.into(),
            airtight: false,
            revoked: false,
            actor: actor.into(),
            created_at: now,
        })
    }
    pub fn get_plan_revision(&self, plan_id: &str, revision: i64) -> Result<PlanRevision> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT plan_id,revision,parent_revision,payload,airtight,revoked,actor,created_at FROM plan_revisions WHERE plan_id=?1 AND revision=?2",rusqlite::params![plan_id,revision],|r| Ok(PlanRevision{plan_id:r.get(0)?,revision:r.get(1)?,parent_revision:r.get(2)?,payload:r.get(3)?,airtight:r.get::<_,i64>(4)?!=0,revoked:r.get::<_,i64>(5)?!=0,actor:r.get(6)?,created_at:r.get(7)?}))?)
    }
    pub fn revoke_plan_airtight(&self, plan_id: &str, revision: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE plan_revisions SET airtight=0,revoked=1 WHERE plan_id=?1 AND revision=?2",
            rusqlite::params![plan_id, revision],
        )?;
        Ok(())
    }

    pub fn add_plan_qa_citation(
        &self,
        plan_id: &str,
        revision: i64,
        step: i64,
        qa_id: &str,
        quote: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO plan_qa_citations(plan_id,revision,step,qa_id,quote) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![plan_id,revision,step,qa_id,quote])?;
        Ok(())
    }
    pub fn save_steering_run(
        &self,
        session_id: &str,
        mode: SteeringMode,
        model: &str,
        vision: &str,
    ) -> Result<SteeringRun> {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let (m, a) = match mode {
            SteeringMode::Normal => ("normal", "plan"),
            SteeringMode::Super => ("super", "all-soft-prompts"),
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO steering_runs VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
            rusqlite::params![&id, session_id, m, model, vision, a, "running", now],
        )?;
        Ok(SteeringRun {
            id,
            session_id: session_id.into(),
            mode,
            model: model.into(),
            vision: vision.into(),
            authority: a.into(),
            status: "running".into(),
            created_at: now,
            updated_at: now,
        })
    }
}
