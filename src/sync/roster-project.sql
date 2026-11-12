-- agmsg v1.7.0 registry projection, MIT; see vendor/agmsg/LICENSE.
    WITH
    source(doc) AS (SELECT ?1),
    raw_records(physical_ord,event) AS (
      SELECT CAST(key AS INTEGER), value FROM source, json_each(source.doc)
    ),
    sync_order AS (
      SELECT json_extract(event,'$.mutation_id') AS mutation_id,
             MIN(CAST(json_extract(event,'$.server_seq') AS INTEGER)) AS server_seq
        FROM raw_records
       WHERE json_extract(event,'$.type')='roster_synced'
       GROUP BY json_extract(event,'$.mutation_id')
    ),
    records(ord,event) AS (
      SELECT row_number() OVER (
               ORDER BY CASE WHEN s.server_seq IS NULL THEN 1 ELSE 0 END,
                        s.server_seq,r.physical_ord
             ) - 1,
             r.event
        FROM raw_records r
        LEFT JOIN sync_order s
          ON s.mutation_id=json_extract(r.event,'$.id')
       WHERE json_extract(r.event,'$.type')
             IN ('member_joined','member_left','member_renamed')
    ),
    normalized AS (
      SELECT ord,json_extract(event,'$.type') AS type,
             json_extract(event,'$.member_id') AS member_id,
             json_extract(event,'$.name') AS name,
             json_extract(event,'$.reuse_name') AS reuse_name,
             json_extract(event,'$.from') AS old_name,
             json_extract(event,'$.to') AS new_name
        FROM records
    ),
    fold(ord,owners,current_names,active) AS (
      SELECT -1,json_object(),json_object(),json_object()
      UNION ALL
      SELECT n.ord,
        CASE
          WHEN n.type='member_joined'
           AND (json_extract(f.owners,'$.' || json_quote(n.name)) IS NULL
             OR (json_extract(f.owners,'$.' || json_quote(n.name))=n.member_id
               AND json_extract(f.active,'$.' || json_quote(n.member_id))=0
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.name)
             OR (n.reuse_name=1
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id)) IS NULL
               AND json_extract(f.active,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=0
               AND json_extract(f.current_names,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=n.name))
            THEN json_set(f.owners,'$.' || json_quote(n.name),n.member_id)
          WHEN n.type='member_renamed'
           AND json_extract(f.active,'$.' || json_quote(n.member_id))=1
           AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.old_name
           AND COALESCE(json_extract(f.owners,'$.' || json_quote(n.new_name)),
                        n.member_id)=n.member_id
            THEN json_set(f.owners,'$.' || json_quote(n.new_name),n.member_id)
          ELSE f.owners
        END,
        CASE
          WHEN n.type='member_joined'
           AND (json_extract(f.owners,'$.' || json_quote(n.name)) IS NULL
             OR (json_extract(f.owners,'$.' || json_quote(n.name))=n.member_id
               AND json_extract(f.active,'$.' || json_quote(n.member_id))=0
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.name)
             OR (n.reuse_name=1
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id)) IS NULL
               AND json_extract(f.active,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=0
               AND json_extract(f.current_names,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=n.name))
            THEN json_set(f.current_names,'$.' || json_quote(n.member_id),n.name)
          WHEN n.type='member_renamed'
           AND json_extract(f.active,'$.' || json_quote(n.member_id))=1
           AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.old_name
           AND COALESCE(json_extract(f.owners,'$.' || json_quote(n.new_name)),
                        n.member_id)=n.member_id
            THEN json_set(f.current_names,'$.' || json_quote(n.member_id),n.new_name)
          ELSE f.current_names
        END,
        CASE
          WHEN n.type='member_joined'
           AND (json_extract(f.owners,'$.' || json_quote(n.name)) IS NULL
             OR (json_extract(f.owners,'$.' || json_quote(n.name))=n.member_id
               AND json_extract(f.active,'$.' || json_quote(n.member_id))=0
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.name)
             OR (n.reuse_name=1
               AND json_extract(f.current_names,'$.' || json_quote(n.member_id)) IS NULL
               AND json_extract(f.active,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=0
               AND json_extract(f.current_names,'$.' || json_quote(json_extract(f.owners,'$.' || json_quote(n.name))))=n.name))
            THEN json_set(f.active,'$.' || json_quote(n.member_id),1)
          WHEN n.type='member_left'
           AND json_extract(f.active,'$.' || json_quote(n.member_id))=1
           AND json_extract(f.current_names,'$.' || json_quote(n.member_id))=n.name
            THEN json_set(f.active,'$.' || json_quote(n.member_id),0)
          ELSE f.active
        END
      FROM fold f JOIN normalized n ON n.ord=f.ord+1
    ),
    final AS (
      SELECT owners,current_names,active FROM fold ORDER BY ord DESC LIMIT 1
    ),
    existing AS (
      SELECT key AS name,value,
             json_extract(value,'$.member_id') AS member_id
        FROM json_each(json_extract(
          ?2, '$.agents'))
    ),
    projected AS (
      SELECT c.value AS name,c.key AS member_id,
             COALESCE(
               (SELECT e.value FROM existing e
                 WHERE e.member_id=c.key LIMIT 1),
               (SELECT json_set(e.value,'$.member_id',c.key)
                  FROM existing e
                 WHERE e.name=c.value LIMIT 1),
               json_object('member_id',c.key,'registrations',json_array())
             ) AS agent
        FROM final f,json_each(f.current_names) c
       WHERE json_extract(f.active,'$.' || json_quote(c.key))=1
         AND json_extract(f.owners,'$.' || json_quote(c.value))=c.key
    ),
    agents(value) AS (
      SELECT COALESCE(json_group_object(name,json(agent)),json_object())
        FROM projected
    ),
    retired(value) AS (
      SELECT COALESCE(json_group_object(c.value,
               json(json_object('member_id',c.key))),json_object())
        FROM final f,json_each(f.current_names) c
       WHERE json_extract(f.active,'$.' || json_quote(c.key))=0
         AND json_extract(f.owners,'$.' || json_quote(c.value))=c.key
    ),
    reused(value) AS (
      SELECT COALESCE(json_group_object(name,1),json_object())
        FROM (SELECT DISTINCT name FROM normalized WHERE type='member_joined' AND reuse_name=1)
    )
    SELECT json_set(?2,
                    '$.agents',json(agents.value),
                    '$.retired_members',json(retired.value),
                    '$.reused_names',json(reused.value))
      FROM agents,retired,reused;
