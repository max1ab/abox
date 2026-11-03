-- agmsg v1.7.0 SQLite sync schema, MIT; see vendor/agmsg/LICENSE.
    CREATE TABLE IF NOT EXISTS sync_store_metadata (
      singleton INTEGER PRIMARY KEY CHECK(singleton=1),
      generation TEXT NOT NULL UNIQUE
    );
    CREATE TABLE IF NOT EXISTS sync_bindings (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      push_cursor INTEGER NOT NULL DEFAULT 0,
      transport_cursor TEXT NOT NULL DEFAULT '0',
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation)
    );
    CREATE TABLE IF NOT EXISTS sync_messages (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      local_position INTEGER NOT NULL,
      local_id TEXT NOT NULL,
      wire_id TEXT NOT NULL,
      envelope_v INTEGER NOT NULL,
      cipher TEXT NOT NULL,
      key_id TEXT,
      blob TEXT NOT NULL,
      server_seq TEXT,
      direction TEXT NOT NULL CHECK(direction IN ('push','pull')),
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,local_position),
      UNIQUE(server_instance_id,remote_team_id,protocol_version,wire_id)
    );
    -- The one lookup on this table that the UNIQUE above cannot serve: the
    -- apply conflict guard asks whether a server_seq is already mapped to a
    -- DIFFERENT wire id, so it comes in by (binding, server_seq). Without
    -- this index that guard walked every row of the binding and fetched each
    -- one (rows carry the wire blob), once per imported message: 68.6 ms per
    -- message on a 21,471-row store, 73 percent of the import batch, and the
    -- reason import time grew with the store (#910). sync_quarantine already
    -- has the equivalent path as its second UNIQUE constraint.
    CREATE INDEX IF NOT EXISTS sync_messages_server_seq
      ON sync_messages(server_instance_id,remote_team_id,protocol_version,server_seq);
    CREATE TABLE IF NOT EXISTS sync_quarantine (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      server_seq TEXT NOT NULL,
      wire_id TEXT NOT NULL,
      server_received_at TEXT NOT NULL,
      envelope_v INTEGER NOT NULL,
      cipher TEXT NOT NULL,
      key_id TEXT,
      blob TEXT NOT NULL,
      status TEXT NOT NULL,
      policy_revision TEXT,
      local_security_revision TEXT,
      reason TEXT,
      PRIMARY KEY(server_instance_id,remote_team_id,protocol_version,wire_id),
      UNIQUE(server_instance_id,remote_team_id,protocol_version,server_seq)
    );
    CREATE TABLE IF NOT EXISTS sync_conflicts (
      conflict_id INTEGER PRIMARY KEY AUTOINCREMENT,
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      server_seq TEXT NOT NULL,
      wire_id TEXT NOT NULL,
      envelope_v INTEGER NOT NULL,
      cipher TEXT NOT NULL,
      key_id TEXT,
      blob TEXT NOT NULL,
      reason TEXT NOT NULL,
      observed_at TEXT NOT NULL,
      UNIQUE(server_instance_id,remote_team_id,protocol_version,
             server_seq,wire_id,reason)
    );
    CREATE TABLE IF NOT EXISTS sync_resync_audits (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      expected_transport_cursor TEXT NOT NULL,
      accepted_floor TEXT NOT NULL,
      gap_start TEXT NOT NULL,
      gap_end TEXT NOT NULL,
      reason TEXT NOT NULL CHECK(reason='retention-gap-accepted'),
      accepted_at TEXT NOT NULL,
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,accepted_floor)
    );
    CREATE TABLE IF NOT EXISTS sync_read_members (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      member_id TEXT NOT NULL,
      agent TEXT NOT NULL,
      remote_agent TEXT NOT NULL,
      active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
      name_mismatch INTEGER NOT NULL DEFAULT 0 CHECK(name_mismatch IN (0,1)),
      blocked_reason TEXT,
      remote_server_seq TEXT NOT NULL DEFAULT '0',
      min_available_seq TEXT NOT NULL DEFAULT '0',
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,member_id),
      UNIQUE(local_team,server_instance_id,remote_team_id,
             protocol_version,driver_generation,agent)
    );
    CREATE TABLE IF NOT EXISTS sync_read_remote_exact (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      member_id TEXT NOT NULL,
      wire_id TEXT NOT NULL,
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,member_id,wire_id)
    );
    CREATE TABLE IF NOT EXISTS sync_read_aliases (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      agent TEXT NOT NULL,
      local_id TEXT NOT NULL,
      wire_id TEXT NOT NULL,
      server_seq TEXT,
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,agent,local_id),
      UNIQUE(server_instance_id,remote_team_id,protocol_version,agent,wire_id)
    );
    CREATE TABLE IF NOT EXISTS sync_read_prepared (
      local_team TEXT NOT NULL,
      server_instance_id TEXT NOT NULL,
      remote_team_id TEXT NOT NULL,
      protocol_version INTEGER NOT NULL,
      driver_generation TEXT NOT NULL,
      member_id TEXT NOT NULL,
      server_seq TEXT NOT NULL,
      PRIMARY KEY(local_team,server_instance_id,remote_team_id,
                  protocol_version,driver_generation,member_id)
    );
