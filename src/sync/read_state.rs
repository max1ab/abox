//! Read-state synchronization for stable, matching roster identities. Frontiers
//! only cover a completely observed prefix; out-of-order reads use exact IDs.
use super::{Context, Result, sequence, sql, uuid};
use reqwest::Method;
use rusqlite::{TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
};

impl Context {
    pub(super) async fn read_state(&mut self) -> Result<()> {
        let roster = self.request(Method::GET, "v1/members", None).await?;
        let members = roster["members"]
            .as_array()
            .filter(|m| m.len() <= 1000)
            .ok_or("invalid remote read roster")?;
        let floor = roster["min_available_seq"]
            .as_str()
            .ok_or("missing read retention floor")?;
        let floor = sequence(floor)?;
        let mut remote = HashMap::new();
        for member in members {
            let id = member["member_id"]
                .as_str()
                .ok_or("missing read member identity")?;
            let name = member["name"].as_str().ok_or("missing read member name")?;
            if !uuid(id, 7)
                || !super::valid_name(name)
                || remote.insert(id.to_owned(), name.to_owned()).is_some()
            {
                return Err("invalid or duplicate read roster member".into());
            }
        }
        let updates = self.prepare_reads(&remote, floor)?;
        let mut after = Value::Null;
        let mut seen = HashSet::new();
        let mut previous: Option<(String, u8, String)> = None;
        for page_number in 0..66538 {
            let page=self.request(Method::POST,"v1/read-state/sync",Some(json!({
                "updates":if page_number==0 {updates.clone()} else {Vec::new()},"page_after":after,"page_limit":1000
            }))).await?;
            let current = sequence(
                page["current_seq"]
                    .as_str()
                    .ok_or("missing read-state current sequence")?,
            )?;
            let page_floor = sequence(
                page["min_available_seq"]
                    .as_str()
                    .ok_or("missing read-state retention floor")?,
            )?;
            if page_floor > current {
                return Err("invalid read-state retention boundary".into());
            }
            let items = page["items"]
                .as_array()
                .filter(|a| a.len() <= 1000)
                .ok_or("invalid read-state page")?;
            for item in items {
                let member = item["member_id"]
                    .as_str()
                    .filter(|id| remote.contains_key(*id))
                    .ok_or("read-state page contains an unknown member")?;
                let key = match item["kind"].as_str() {
                    Some("frontier") => {
                        let seq =
                            sequence(item["server_seq"].as_str().ok_or("missing read frontier")?)?;
                        if seq > current || !seen.insert(member.to_owned()) {
                            return Err("invalid or repeated read frontier".into());
                        }
                        (member.to_owned(), 0, String::new())
                    }
                    Some("exact") => {
                        let wire = item["wire_id"]
                            .as_str()
                            .filter(|wire| uuid(wire, 4))
                            .ok_or("invalid exact read ID")?;
                        (member.to_owned(), 1, wire.to_owned())
                    }
                    _ => return Err("unknown read-state item".into()),
                };
                if previous.as_ref().is_some_and(|p| p >= &key) {
                    return Err("read-state pagination is not ordered".into());
                }
                previous = Some(key);
            }
            let more = page["has_more"]
                .as_bool()
                .ok_or("missing read-state pagination flag")?;
            if more {
                let last = items
                    .last()
                    .ok_or("empty read-state page claims more data")?;
                let expected = if last["kind"] == "frontier" {
                    json!({"member_id":last["member_id"],"kind":"frontier"})
                } else {
                    json!({"member_id":last["member_id"],"kind":"exact","wire_id":last["wire_id"]})
                };
                if page["next_page_after"] != expected {
                    return Err("read-state pagination cursor mismatch".into());
                }
                after = expected;
            } else if !page["next_page_after"].is_null() {
                return Err("unexpected final read-state cursor".into());
            }
            self.apply_reads(items, page_floor)?;
            if !more {
                if seen.len() != remote.len() {
                    return Err("read-state snapshot omitted a member frontier".into());
                }
                return Ok(());
            }
        }
        Err("read-state pagination exceeds protocol bounds".into())
    }
    fn prepare_reads(
        &mut self,
        members: &HashMap<String, String>,
        floor: i64,
    ) -> Result<Vec<Value>> {
        let _guard = self.guard()?;
        let config: Value = serde_json::from_slice(
            &fs::read(self.store.team_dir.join("config.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let local = config["agents"]
            .as_object()
            .ok_or("invalid local read roster")?;
        let cursor = sequence(&self.cursor()?)?
            .min(sequence(&super::roster::Roster::load(self)?.cursor()?)?);
        let tx = sql(self
            .store
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        sql(tx.execute("UPDATE sync_read_members SET active=0 WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4",params![self.team,self.server,self.remote,self.generation]))?;
        let mut updates = Vec::new();
        let mut budget = 1000usize;
        for (id, name) in members {
            // A name changed independently of the server snapshot is not enough
            // evidence to move another identity's reads. Match both name and ID.
            let matched = local
                .get(name)
                .is_some_and(|member| member["member_id"] == *id);
            sql(tx.execute("INSERT INTO sync_read_members(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,member_id,agent,remote_agent,active,name_mismatch,remote_server_seq,min_available_seq)
              VALUES (?1,?2,?3,1,?4,?5,?6,?6,1,?7,?8,?8)
              ON CONFLICT(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,member_id) DO UPDATE SET remote_agent=excluded.remote_agent,active=1,name_mismatch=CASE WHEN sync_read_members.agent=excluded.agent THEN excluded.name_mismatch ELSE 1 END,min_available_seq=CAST(MAX(CAST(sync_read_members.min_available_seq AS INTEGER),?8) AS TEXT),remote_server_seq=CAST(MAX(CAST(sync_read_members.remote_server_seq AS INTEGER),?8) AS TEXT)",params![self.team,self.server,self.remote,self.generation,id,name,if matched {0}else{1},floor.to_string()]))?;
            let mismatch:bool=sql(tx.query_row("SELECT name_mismatch FROM sync_read_members WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4 AND member_id=?5",params![self.team,self.server,self.remote,self.generation,id],|r|r.get(0)))?;
            if mismatch {
                continue;
            }
            let first_unread:Option<i64>=sql(tx.query_row("SELECT MIN(CAST(m.server_seq AS INTEGER)) FROM sync_messages m JOIN events e ON e.seq=m.local_position
              WHERE m.local_team=?1 AND m.server_instance_id=?2 AND m.remote_team_id=?3 AND m.protocol_version=1 AND m.driver_generation=?4
              AND m.server_seq IS NOT NULL AND e.to_agent=?5 AND CAST(m.server_seq AS INTEGER)>?6
              AND e.seq>COALESCE((SELECT local_position FROM read_cursors WHERE team=?1 AND agent=?5),0)
              AND NOT EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=?5 AND r.msg_id=e.id)",params![self.team,self.server,self.remote,self.generation,name,floor],|r|r.get(0)))?;
            let first_unresolved:Option<i64>=sql(tx.query_row("SELECT MIN(CAST(server_seq AS INTEGER)) FROM sync_quarantine WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4 AND CAST(server_seq AS INTEGER)>?5 AND status NOT IN ('imported','reconciled')",params![self.team,self.server,self.remote,self.generation,floor],|r|r.get(0)))?;
            let mut frontier = cursor.max(floor);
            for gap in [first_unread, first_unresolved].into_iter().flatten() {
                frontier = frontier.min(gap.saturating_sub(1));
            }
            // Incomplete/corrupt prior state cannot justify skipping an unseen
            // message, even when a transport cursor says a page was traversed.
            let known:i64=sql(tx.query_row("SELECT COUNT(DISTINCT seq) FROM (
              SELECT CAST(server_seq AS INTEGER) AS seq FROM sync_messages WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4
              UNION SELECT CAST(server_seq AS INTEGER) FROM sync_quarantine WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4)
              WHERE seq>?5 AND seq<=?6",params![self.team,self.server,self.remote,self.generation,floor,frontier],|r|r.get(0)))?;
            if known != frontier - floor {
                frontier = floor;
            }
            let exact = {
                let mut statement=sql(tx.prepare("SELECT DISTINCT m.wire_id FROM sync_messages m JOIN events e ON e.seq=m.local_position JOIN sync_read_members rm ON rm.local_team=m.local_team AND rm.server_instance_id=m.server_instance_id AND rm.remote_team_id=m.remote_team_id AND rm.protocol_version=m.protocol_version AND rm.driver_generation=m.driver_generation AND rm.member_id=?5
                  WHERE m.local_team=?1 AND m.server_instance_id=?2 AND m.remote_team_id=?3 AND m.protocol_version=1 AND m.driver_generation=?4 AND m.server_seq IS NOT NULL AND e.to_agent=?6
                  AND CAST(m.server_seq AS INTEGER)>MAX(?7,CAST(rm.remote_server_seq AS INTEGER))
                  AND (e.seq<=COALESCE((SELECT local_position FROM read_cursors WHERE team=?1 AND agent=?6),0)
                    OR EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=?6 AND r.msg_id=e.id))
                  AND NOT EXISTS(SELECT 1 FROM sync_read_remote_exact x WHERE x.local_team=m.local_team AND x.server_instance_id=m.server_instance_id AND x.remote_team_id=m.remote_team_id AND x.protocol_version=m.protocol_version AND x.driver_generation=m.driver_generation AND x.member_id=?5 AND x.wire_id=m.wire_id)
                  ORDER BY m.wire_id LIMIT ?8"))?;
                sql(statement.query_map(
                    params![
                        self.team,
                        self.server,
                        self.remote,
                        self.generation,
                        id,
                        name,
                        frontier,
                        budget as i64
                    ],
                    |r| r.get::<_, String>(0),
                ))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?
            };
            budget -= exact.len();
            updates.push(
                json!({"member_id":id,"server_seq":frontier.to_string(),"exact_wire_ids":exact}),
            );
        }
        sql(tx.execute("INSERT OR IGNORE INTO sync_read_aliases(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,agent,local_id,wire_id,server_seq)
          SELECT m.local_team,m.server_instance_id,m.remote_team_id,m.protocol_version,m.driver_generation,e.to_agent,m.local_id,m.wire_id,m.server_seq FROM sync_messages m JOIN events e ON e.seq=m.local_position WHERE m.local_team=?1 AND m.server_instance_id=?2 AND m.remote_team_id=?3 AND m.protocol_version=1 AND m.driver_generation=?4 AND m.server_seq IS NOT NULL",params![self.team,self.server,self.remote,self.generation]))?;
        sql(tx.commit())?;
        Ok(updates)
    }
    fn apply_reads(&mut self, items: &[Value], floor: i64) -> Result<()> {
        let _guard = self.guard()?;
        let tx = sql(self
            .store
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        for item in items {
            let id = item["member_id"].as_str().ok_or("invalid read member")?;
            if item["kind"] == "frontier" {
                let seq = item["server_seq"].as_str().ok_or("missing read frontier")?;
                sql(tx.execute("UPDATE sync_read_members SET remote_server_seq=CAST(MAX(CAST(remote_server_seq AS INTEGER),?6) AS TEXT) WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4 AND member_id=?5 AND active=1",params![self.team,self.server,self.remote,self.generation,id,sequence(seq)?]))?;
            } else {
                sql(tx.execute("INSERT OR IGNORE INTO sync_read_remote_exact(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,member_id,wire_id) VALUES (?1,?2,?3,1,?4,?5,?6)",params![self.team,self.server,self.remote,self.generation,id,item["wire_id"].as_str().ok_or("missing exact read ID")?]))?;
            }
        }
        sql(tx.execute("UPDATE sync_read_members SET remote_server_seq=CAST(MAX(CAST(remote_server_seq AS INTEGER),?5) AS TEXT),min_available_seq=CAST(MAX(CAST(min_available_seq AS INTEGER),?5) AS TEXT) WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4",params![self.team,self.server,self.remote,self.generation,floor]))?;
        let reads = {
            let mut statement=sql(tx.prepare("SELECT DISTINCT e.id,e.to_agent,e.legacy_id FROM sync_messages m JOIN events e ON e.seq=m.local_position JOIN sync_read_members rm ON rm.local_team=m.local_team AND rm.server_instance_id=m.server_instance_id AND rm.remote_team_id=m.remote_team_id AND rm.protocol_version=m.protocol_version AND rm.driver_generation=m.driver_generation AND rm.agent=e.to_agent
              WHERE m.local_team=?1 AND m.server_instance_id=?2 AND m.remote_team_id=?3 AND m.protocol_version=1 AND m.driver_generation=?4 AND m.server_seq IS NOT NULL AND rm.active=1 AND rm.name_mismatch=0
              AND (CAST(m.server_seq AS INTEGER)<=CAST(rm.remote_server_seq AS INTEGER) OR EXISTS(SELECT 1 FROM sync_read_remote_exact x WHERE x.local_team=m.local_team AND x.server_instance_id=m.server_instance_id AND x.remote_team_id=m.remote_team_id AND x.protocol_version=m.protocol_version AND x.driver_generation=m.driver_generation AND x.member_id=rm.member_id AND x.wire_id=m.wire_id))
              AND NOT EXISTS(SELECT 1 FROM sync_quarantine q WHERE q.server_instance_id=m.server_instance_id AND q.remote_team_id=m.remote_team_id AND q.protocol_version=m.protocol_version AND q.wire_id=m.wire_id AND q.status NOT IN ('imported','reconciled'))
              AND NOT EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=e.to_agent AND r.msg_id=e.id)"))?;
            sql(statement.query_map(
                params![self.team, self.server, self.remote, self.generation],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                },
            ))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
        };
        for (id, agent, legacy) in reads {
            sql(tx.execute("INSERT INTO events(type,id,team,agent,msg_id,at) VALUES ('message_read',?1,?2,?3,?4,strftime('%Y-%m-%dT%H:%M:%SZ','now'))",params![uuid::Uuid::now_v7().to_string(),self.team,agent,id]))?;
            if let Some(legacy) = legacy {
                sql(tx.execute("UPDATE messages SET read_at=COALESCE(read_at,strftime('%Y-%m-%dT%H:%M:%SZ','now')) WHERE id=?1",[legacy]))?;
            }
        }
        sql(tx.execute("DELETE FROM sync_read_remote_exact AS x WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4 AND EXISTS(SELECT 1 FROM sync_read_members rm JOIN sync_messages m ON m.local_team=rm.local_team AND m.server_instance_id=rm.server_instance_id AND m.remote_team_id=rm.remote_team_id AND m.protocol_version=rm.protocol_version AND m.driver_generation=rm.driver_generation WHERE rm.member_id=x.member_id AND rm.local_team=x.local_team AND rm.server_instance_id=x.server_instance_id AND rm.remote_team_id=x.remote_team_id AND rm.protocol_version=x.protocol_version AND rm.driver_generation=x.driver_generation AND m.wire_id=x.wire_id AND CAST(m.server_seq AS INTEGER)<=CAST(rm.remote_server_seq AS INTEGER))",params![self.team,self.server,self.remote,self.generation]))?;
        sql(tx.commit())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, incoming, page};
    use super::*;
    #[test]
    fn frontiers_do_not_skip_unread_or_unobserved_rows_and_exact_reads_are_scoped() {
        let temp = fixture();
        let mut ctx = Context::open(temp.path(), "dev").unwrap();
        let first = incoming();
        let second = incoming();
        ctx.apply("0", "0", &page(&first, "1")).unwrap();
        ctx.apply("1", "1", &page(&second, "2")).unwrap();
        let receiver = "019a4000-0000-7000-8000-000000000004".to_owned();
        let sender = "019a4000-0000-7000-8000-000000000003".to_owned();
        let members = HashMap::from([
            (receiver.clone(), "receiver".into()),
            (sender.clone(), "sender".into()),
        ]);
        let updates = ctx.prepare_reads(&members, 0).unwrap();
        assert_eq!(
            updates.iter().find(|u| u["member_id"] == receiver).unwrap()["server_seq"],
            "0"
        );
        assert_eq!(
            updates.iter().find(|u| u["member_id"] == sender).unwrap()["server_seq"],
            "2"
        );
        ctx.apply_reads(
            &[json!({"kind":"exact","member_id":receiver,"wire_id":first.id})],
            0,
        )
        .unwrap();
        ctx.apply_reads(
            &[json!({"kind":"exact","member_id":receiver,"wire_id":first.id})],
            0,
        )
        .unwrap();
        assert_eq!(
            ctx.store
                .db
                .query_row(
                    "SELECT count(*) FROM events WHERE type='message_read'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            ctx.store
                .db
                .query_row(
                    "SELECT count(*) FROM messages WHERE read_at IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let updates = ctx.prepare_reads(&members, 0).unwrap();
        assert_eq!(
            updates.iter().find(|u| u["member_id"] == receiver).unwrap()["server_seq"],
            "1"
        );
        // An earned cursor without all of its mappings is insufficient proof
        // to mark the missing prefix read on another machine.
        ctx.store
            .db
            .execute("UPDATE sync_bindings SET transport_cursor='3'", [])
            .unwrap();
        let _guard = ctx.guard().unwrap();
        let mut roster = super::super::roster::Roster::load(&ctx).unwrap();
        roster.advance("3").unwrap();
        roster.save().unwrap();
        drop(_guard);
        let updates = ctx.prepare_reads(&members, 0).unwrap();
        assert_eq!(
            updates.iter().find(|u| u["member_id"] == sender).unwrap()["server_seq"],
            "0"
        );
    }
}
