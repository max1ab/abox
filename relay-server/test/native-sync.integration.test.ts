import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Pool } from "pg";
import { describe, expect, it } from "vitest";
import { migrate } from "@agmsg/reference-server/dist/src/db.js";
import { createRelayApp } from "../src/app.js";
import { migrateEvents } from "../src/events.js";
import { migrateCollaboration } from "../src/collaboration.js";

const enabled=process.env.TEST_DATABASE_URL && process.env.AGENT_MAIL_TEST_BIN && process.env.AGENT_MAIL_TEST_AGMSG_DIR;
function run(file: string, args: string[], env: NodeJS.ProcessEnv, input=""): Promise<string> {
  return new Promise((resolve,reject)=>{
    const child=spawn(file,args,{env,stdio:["pipe","pipe","pipe"]});
    let out="",err="";
    const timer=setTimeout(()=>{child.kill();reject(new Error("test child timed out"));},20000);
    child.stdout.on("data",d=>{out+=d;});child.stderr.on("data",d=>{err+=d;});
    child.once("error",e=>{clearTimeout(timer);reject(e);});
    child.once("close",code=>{clearTimeout(timer);code===0?resolve(out):reject(new Error(`child ${code}: ${err}`));});
    child.stdin.end(input);
  });
}
(enabled?describe:describe.skip)("native Rust sync against official PostgreSQL protocol and actual agmsg readers",()=>{
  it("takes over official reservations, tolerates a lost ack, and delivers once to another mailbox",async()=>{
    const databaseUrl=process.env.TEST_DATABASE_URL!;
    const schema=`agent_mail_test_${randomBytes(8).toString("hex")}`;
    const admin=new Pool({connectionString:databaseUrl});
    let pool:Pool|undefined;
    let app:ReturnType<typeof createRelayApp>|undefined;
    const temp=await mkdtemp(join(tmpdir(),"agent-mail-native-sync-"));
    let created=false;
    let resident:ChildProcess|undefined;
    let receiving:ChildProcess|undefined;
    let managed=false;
    try {
      await admin.query(`CREATE SCHEMA ${schema}`);created=true;
      pool=new Pool({connectionString:databaseUrl,options:`-c search_path=${schema}`,application_name:schema});
      await migrate(pool);await migrateCollaboration(pool);await migrateEvents(pool);
      app=createRelayApp(pool,{databaseUrl,host:"127.0.0.1",port:8787,logLevel:"silent",retentionMaxLiveMessages:null});
      let refuseUploads=false;
      let refuseEvents=false;
      let refuseProfiles=false;
      app.addHook("onRequest",async(request,reply)=>{
        if(refuseProfiles && request.url.startsWith("/v1/directory/profiles")) return reply.code(503).send({error:"test profile outage"});
        if(refuseEvents && request.url==="/v1/agent-mail/events") return reply.code(503).send({error:"test disconnected stream"});
        if(refuseUploads && request.method==="POST" && request.url==="/v1/messages") return reply.code(503).send({error:"test transient failure"});
      });
      const address=await app.listen({host:"127.0.0.1",port:0});
      const a=join(temp,"a"),b=join(temp,"b");
      await mkdir(a);
      await cp(join(process.env.AGENT_MAIL_TEST_AGMSG_DIR!,"scripts"),join(a,"scripts"),{recursive:true});
      await cp(join(process.env.AGENT_MAIL_TEST_AGMSG_DIR!,"VERSION"),join(a,"VERSION"));
      const env={...process.env,AGMSG_CONFIG:join(temp,"global-config.json"),AGMSG_STORAGE_DRIVER:"sqlite",AGMSG_RESOLVE_PROJECT:"0",AGMSG_SELF_NAME:"off",AGMSG_SELF_RENAME:"off"};
      for(const key of ["AGMSG_STORAGE_PATH","AGMSG_PLUGIN_DIRS","SKILL_DIR","CODEX_THREAD_ID","CODEX_SESSION_ID","AGENT_MAIL_BASH"]) delete env[key as keyof typeof env];
      for(const agent of ["sender","receiver"]) await run("bash",[join(a,"scripts/join.sh"),"contract",agent,"codex",a],env);
      await cp(a,b,{recursive:true});
      for(const root of [a,b]) await run("bash",["-c",'source "$1/scripts/lib/storage.sh"; agmsg_storage_load; storage_init contract',"fixture",root],env);
      const config=JSON.parse(await readFile(join(a,"teams/contract/config.json"),"utf8"));
      const team=config.team_id;
      const headers={"agmsg-protocol-version":"1","agmsg-team-id":team};
      const connected=await app.inject({method:"POST",url:"/v1/connect",headers,payload:{team_id:team,team_name:"contract",cipher_profile:"none",members:Object.entries(config.agents).map(([name,member])=>({name,member_id:(member as {member_id:string}).member_id}))}});
      expect(connected.statusCode).toBe(200);
      const remote=connected.json();
      config.remote_binding={endpoint:address,server_instance_id:remote.server_instance_id,remote_team_id:team,protocol_version:1,cipher_profile:"none",disconnected_at:null};
      for(const root of [a,b]) await writeFile(join(root,"teams/contract/config.json"),JSON.stringify(config));
      const cli=resolve(process.env.AGENT_MAIL_TEST_BIN!);
      const privateMessages = "WHERE NOT (convert_from(decode(blob,'base64'),'UTF8')::jsonb ? 'kind')";
      const invoke=(root:string,args:string[],input="")=>run(cli,["--server",address,"--agmsg-dir",root,...args],env,input);
      const boardBody="结论 @receiver\nquotes: ' \"\n";
      const post=JSON.parse(await invoke(a,["board","post","contract","--author","sender","--stdin"],boardBody));
      expect(post.body).toBe(boardBody);
      expect(JSON.parse(await invoke(join(temp,"no-agmsg"),["board","list","contract"]))).toEqual(post);
      expect(JSON.parse(await invoke(a,["board","list","contract"]))).toEqual(post);
      await invoke(a,["directory","set","contract","sender","--display-name","Sender","--description","角色简介"]);
      expect(JSON.parse(await invoke(a,["directory","get","contract","sender"])).description).toBe("角色简介");
      const directory=(await invoke(a,["directory","list","contract"])).trim().split("\n").map(line=>JSON.parse(line));
      expect(directory.find(row=>row.agent==="sender").display_name).toBe("Sender");
      expect(directory.find(row=>row.agent==="receiver").display_name).toBe(null);
      refuseProfiles=true;
      await expect(invoke(a,["directory","list","contract"])).rejects.toThrow("503");
      refuseProfiles=false;
      const checks=(await invoke(a,["doctor"])).trim().split("\n").map(line=>JSON.parse(line));
      expect(checks).toHaveLength(2);expect(checks.every(check=>check.ok)).toBe(true);
      const body="中文 private mail\nquotes: ' \" @receiver";
      await invoke(a,["agmsg","send","contract","--from","sender","--to","receiver","--stdin"],body);
      // Reserve with the upstream driver before switching engines.
      const prepared=await run("bash",[join(a,"scripts/internal/storage-sync-driver.sh"),"prepare","contract",remote.server_instance_id,team,"1","10"],env,
        JSON.stringify({type:"sync_prepare",envelope_v:1,cipher:"none",key_id:null,recipients:[],max_blob_bytes:1048576,allow_new:true})+"\n");
      const candidates=prepared.trim().split("\n").map(line=>JSON.parse(line)).filter(r=>r.type==="sync_push_candidate");
      expect(candidates).toHaveLength(1);
      // Commit remotely but deliberately do not reconcile locally (lost ACK).
      const {id,envelope}=candidates[0];
      const stored=await app.inject({method:"POST",url:"/v1/messages",headers,payload:{messages:[{id,envelope}]}});
      expect(stored.json().acks[0].disposition).toBe("stored");
      const takeover=JSON.parse(await invoke(a,["sync","once","contract"]));
      expect(takeover.uploaded).toBe(1);
      expect(takeover.downloaded).toBe(0);
      expect((await pool.query(`SELECT id::text FROM messages ${privateMessages}`)).rows).toEqual([{id}]);
      expect(JSON.parse(await invoke(b,["sync","once","contract"])).downloaded).toBe(1);
      expect(JSON.parse(await invoke(b,["sync","once","contract"])).downloaded).toBe(0);
      expect(JSON.parse(await invoke(b,["sync","once","contract"])).uploaded).toBe(0);
      const inbox=await invoke(b,["agmsg","inbox","contract","receiver"]);
      for(const line of body.split("\n")) expect(inbox).toContain(line);
      const history=(await invoke(b,["agmsg","messages","contract"])).trim().split("\n").map(line=>JSON.parse(line));
      expect(history).toHaveLength(1);
      expect(history[0].body).toBe(body);
      expect(await invoke(b,["agmsg","inbox","contract","receiver"])).not.toContain("1 new message");
      await invoke(b,["sync","once","contract"]);
      await invoke(a,["sync","once","contract"]);
      // The same identity opened on another machine must see the read state,
      // rather than deliver the old task a second time.
      expect(await invoke(a,["agmsg","inbox","contract","receiver"])).not.toContain("1 new message");

      expect((await pool.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count).toBe("1");
      const baselineCursor=(await pool.query("SELECT current_seq::text FROM teams WHERE team_id=$1",[team])).rows[0].current_seq;
      // A 60-second fallback must not delay a native send: its IPC hint should
      // trigger upload while the process remains otherwise idle.
      resident=spawn(cli,["--agmsg-dir",a,"sync","run","contract","--interval","60"],{env,stdio:"ignore"});
      const waitFor=async(check:()=>Promise<boolean>)=>{
        const deadline=Date.now()+8000;
        do {if(await check()) return;await new Promise(resolve=>setTimeout(resolve,50));} while(Date.now()<deadline);
        throw new Error("condition did not become true before timeout");
      };
      await waitFor(async()=>JSON.parse(await invoke(a,["sync","status","contract"])).running);
      await expect(invoke(a,["sync","once","contract"])).rejects.toThrow("already running");
      receiving=spawn(cli,["--agmsg-dir",b,"sync","run","contract","--interval","60"],{env,stdio:"ignore"});
      await waitFor(async()=>{
        const status=JSON.parse(await invoke(b,["sync","status","contract"]));
        return status.remote_events==="connected" && status.phase==="idle" && status.transport_cursor===baselineCursor;
      });
      await new Promise(resolve=>setTimeout(resolve,200));

      await invoke(a,["agmsg","send","contract","--from","sender","--to","receiver","--stdin"],"IPC wake test");
      await waitFor(async()=>(await pool!.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count==="2");
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(true);
      await waitFor(async()=>JSON.parse(await invoke(b,["sync","status","contract"])).downloaded>=1);
      const caughtUpCursor=(await pool.query("SELECT current_seq::text FROM teams WHERE team_id=$1",[team])).rows[0].current_seq;
      refuseEvents=true;
      const listeners=await admin.query("SELECT pid FROM pg_stat_activity WHERE application_name=$1",[`${schema}:agent-mail-events`]);
      expect(listeners.rows).toHaveLength(1);
      await admin.query("SELECT pg_terminate_backend($1)",[listeners.rows[0].pid]);
      await waitFor(async()=>{
        const status=JSON.parse(await invoke(b,["sync","status","contract"]));
        return status.remote_events==="fallback" && status.phase==="idle" && status.transport_cursor===caughtUpCursor;
      });
      await new Promise(resolve=>setTimeout(resolve,200));
      await invoke(a,["agmsg","send","contract","--from","sender","--to","receiver","--stdin"],"mail while SSE is disconnected");
      await waitFor(async()=>(await pool!.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count==="3");
      refuseEvents=false;
      await waitFor(async()=>JSON.parse(await invoke(b,["sync","status","contract"])).downloaded>=2);
      refuseUploads=true;
      await invoke(a,["agmsg","send","contract","--from","sender","--to","receiver","--stdin"],"retry keeps the outbox");
      await waitFor(async()=>JSON.parse(await invoke(a,["sync","status","contract"])).phase==="retrying");
      expect((await pool.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count).toBe("3");
      refuseUploads=false;
      await waitFor(async()=>(await pool!.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count==="4");
      const beforeKill=(await pool.query(`SELECT id::text FROM messages ${privateMessages} ORDER BY id`)).rows;
      // Killing the process must release the OS lock even without cleanup.
      resident.kill("SIGKILL");
      await new Promise<void>(resolve=>resident!.once("close",()=>resolve()));
      resident=undefined;
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(false);
      const recovered=JSON.parse(await invoke(a,["sync","once","contract"]));
      // SIGKILL can land after the remote commit but before local ACK storage.
      // Re-uploading that reservation is allowed; creating a new server row is not.
      expect(recovered.uploaded).toBeLessThanOrEqual(1);
      expect((await pool.query(`SELECT id::text FROM messages ${privateMessages} ORDER BY id`)).rows).toEqual(beforeKill);
      resident=spawn(cli,["--agmsg-dir",a,"sync","run","contract","--interval","1"],{env,stdio:"ignore"});
      await waitFor(async()=>JSON.parse(await invoke(a,["sync","status","contract"])).running);
      // The upstream shell has no native IPC hook. Recovery still finds its write.
      await run("bash",[join(a,"scripts/send.sh"),"contract","sender","receiver","message without IPC"],env);
      await waitFor(async()=>(await pool!.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count==="5");
      resident.kill("SIGTERM");
      const exit=await new Promise<number|null>(resolve=>resident!.once("close",code=>resolve(code)));
      expect(exit).toBe(0);resident=undefined;
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(false);
      // Real upstream membership commands must cross the native transport,
      // retaining receiver-local registrations and remaining silent as mail.
      const members=async(root:string)=>(await invoke(root,["agmsg","members","contract"])).trim().split("\n").filter(Boolean).map(line=>JSON.parse(line));
      await run("bash",[join(a,"scripts/join.sh"),"contract","newcomer","codex",a],env);
      await invoke(a,["sync","once","contract"]);
      await waitFor(async()=>(await members(b)).some(member=>member.name==="newcomer"));
      await run("bash",[join(a,"scripts/rename.sh"),"contract","newcomer","renamed"],env);
      await invoke(a,["sync","once","contract"]);
      await waitFor(async()=>(await members(b)).some(member=>member.name==="renamed"));
      const localBefore=JSON.parse(await readFile(join(b,"teams/contract/config.json"),"utf8")).agents.receiver;
      await run("bash",[join(a,"scripts/leave.sh"),"contract","renamed"],env);
      await invoke(a,["sync","once","contract"]);
      await waitFor(async()=>!(await members(b)).some(member=>member.name==="renamed"));
      const localAfter=JSON.parse(await readFile(join(b,"teams/contract/config.json"),"utf8")).agents.receiver;
      expect(localAfter).toEqual(localBefore);
      expect((await pool.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count).toBe("5");

      // Exercise the real official daemon-to-native handoff, rather than a
      // fake pidfile. Only this temporary installation is patched; no login
      // service is registered on the host.
      const connectedConfig=JSON.parse(await readFile(join(a,"teams/contract/config.json"),"utf8"));
      connectedConfig.remote_binding.connected_at=new Date().toISOString();
      connectedConfig.remote_binding.capabilities=remote;
      await writeFile(join(a,"teams/contract/config.json"),JSON.stringify(connectedConfig));
      await run("bash",["-c",'source "$1/scripts/lib/install-manifest.sh"; agmsg_install_manifest_write "$1/scripts" "$1/run/install-manifest.json" v1.7.0 integration-test 1 1',"manifest",a],env);
      managed=true; // Cleanup must run even if adoption fails halfway through.
      try { await run("bash",[join(a,"scripts/remote.sh"),"sync","start","contract"],env); }
      catch(error) {
        const log=await readFile(join(a,"run/remote-sync.contract.log"),"utf8").catch(()=>"no official engine log");
        throw new Error(`${String(error)}\nOfficial temporary engine log:\n${log}`);
      }
      const oldPid=await readFile(join(a,"run/remote-sync.contract.pid"),"utf8");
      expect(oldPid.trim()).toMatch(/^\d+$/);
      const selected=JSON.parse(await invoke(a,["sync","enable","contract","--no-login"]));
      expect(selected.running).toBe(true);
      await expect(readFile(join(a,"run/remote-sync.contract.pid"))).rejects.toThrow();
      await invoke(a,["sync","stop","contract"]);
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(false);
      // Both direct upstream start and the session hook select native now.
      await run("bash",[join(a,"scripts/remote.sh"),"sync","start","contract"],env);
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(true);
      await invoke(a,["agmsg","send","contract","--from","sender","--to","receiver","--stdin"],"native after actual official handoff");
      await waitFor(async()=>(await pool!.query(`SELECT count(*) FROM messages ${privateMessages}`)).rows[0].count==="6");
      await waitFor(async()=>(await invoke(b,["agmsg","messages","contract"])).includes("native after actual official handoff"));
      await invoke(a,["sync","disable","contract"]);managed=false;
      expect(JSON.parse(await invoke(a,["sync","status","contract"])).running).toBe(false);

    } finally {
      if(managed) {
        const env={...process.env};
        for(const key of ["AGMSG_STORAGE_PATH","AGMSG_PLUGIN_DIRS","SKILL_DIR","CODEX_THREAD_ID","CODEX_SESSION_ID","AGENT_MAIL_BASH"]) delete env[key];
        await run(resolve(process.env.AGENT_MAIL_TEST_BIN!),["--agmsg-dir",join(temp,"a"),"sync","disable","contract"],env).catch(()=>{});
      }
      if(receiving && receiving.exitCode===null && receiving.signalCode===null) {receiving.kill("SIGKILL");await new Promise<void>(resolve=>receiving!.once("close",()=>resolve()));}
      if(resident && resident.exitCode===null && resident.signalCode===null) {resident.kill("SIGKILL");await new Promise<void>(resolve=>resident!.once("close",()=>resolve()));}
      await app?.close();await pool?.end();
      if(created) await admin.query(`DROP SCHEMA ${schema} CASCADE`);
      await admin.end();await rm(temp,{recursive:true,force:true});
    }
  },60000);
});
