use super::Db;
use super::domain::*;
use anyhow::{Result, bail};
use uuid::Uuid;

impl Db {
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
