import { randomBytes, randomUUID } from "node:crypto";
import { Pool } from "pg";
import { describe, expect, it } from "vitest";
import { migrate } from "@agmsg/reference-server/dist/src/db.js";
import { createRelayApp } from "../src/app.js";
import { migrateCollaboration } from "../src/collaboration.js";
import { migrateEvents } from "../src/events.js";

const databaseUrl=process.env.TEST_DATABASE_URL;
const pause=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
function bounded<T>(promise:Promise<T>,ms=5000):Promise<T> {
  return new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>reject(new Error("event stream timeout")),ms);
    promise.then(value=>{clearTimeout(timer);resolve(value);},error=>{clearTimeout(timer);reject(error);});
  });
}
(databaseUrl?describe:describe.skip)("transactional SSE signals",()=>{
  it("notifies on commit, stays silent on rollback/board/other teams, and closes on listener loss",async()=>{
    const schema=`agent_mail_test_${randomBytes(8).toString("hex")}`;
    const admin=new Pool({connectionString:databaseUrl});
    let pool:Pool|undefined,app:ReturnType<typeof createRelayApp>|undefined;
    const controllers:AbortController[]=[];
    let created=false;
    try {
      await admin.query(`CREATE SCHEMA ${schema}`);created=true;
      pool=new Pool({connectionString:databaseUrl,options:`-c search_path=${schema}`,application_name:schema});
      await migrate(pool);await migrateCollaboration(pool);await migrateEvents(pool);await migrateEvents(pool);
      app=createRelayApp(pool,{databaseUrl:databaseUrl!,host:"127.0.0.1",port:8787,logLevel:"silent",retentionMaxLiveMessages:null});
      const address=await app.listen({host:"127.0.0.1",port:0});
      const team="019a4000-0000-7000-8000-000000000011",other="019a4000-0000-7000-8000-000000000012";
      const headers={"agmsg-protocol-version":"1","agmsg-team-id":team};
      for(const id of [team,other]) expect((await app.inject({method:"POST",url:"/v1/connect",headers:{...headers,"agmsg-team-id":id},payload:{team_id:id,team_name:id,cipher_profile:"none",members:[{member_id:"019a4000-0000-7000-8000-000000000013",name:"mac"}]}})).statusCode).toBe(200);
      const connect=async()=>{
        const controller=new AbortController();controllers.push(controller);
        const response=await fetch(`${address}/v1/agent-mail/events`,{headers,signal:controller.signal});
        expect(response.status).toBe(200);
        expect(response.headers.get("content-type")).toContain("text/event-stream");
        const reader=response.body!.getReader();
        let buffer="";const decoder=new TextDecoder();
        const next=async():Promise<Record<string,unknown>>=>{
          for(;;) {
            const end=buffer.indexOf("\n\n");
            if(end>=0) {
              const frame=buffer.slice(0,end);buffer=buffer.slice(end+2);
              if(frame.startsWith(":")) continue;
              expect(frame).toContain("event: new_messages");
              return JSON.parse(frame.split("\ndata: ")[1]!);
            }
            const chunk=await reader.read();if(chunk.done) throw new Error("event stream closed");
            buffer+=decoder.decode(chunk.value,{stream:true});
          }
        };
        return {next:()=>bounded(next()),reader};
      };
      const stream=await connect();
      const initial=await stream.next();
      expect(initial).toMatchObject({team_id:team,protocol_version:1});
      const envelope={v:1,cipher:"none",key_id:null,blob:Buffer.from("private body must not travel in SSE").toString("base64")};
      const id=randomUUID();
      let pending=stream.next();
      const send=await app.inject({method:"POST",url:"/v1/messages",headers,payload:{messages:[{id,envelope}]}});
      expect(send.statusCode).toBe(200);
      expect(await pending).toEqual(initial);
      pending=stream.next();
      const conflict=await app.inject({method:"POST",url:"/v1/messages",headers,payload:{messages:[{id:randomUUID(),envelope},{id,envelope:{...envelope,blob:Buffer.from("changed").toString("base64")}}]}});
      expect(conflict.statusCode).toBe(409);
      expect((await app.inject({method:"POST",url:"/v1/board/posts",payload:{team:"dev",author:"mac",body:"@dell silent board"}})).statusCode).toBe(201);
      expect((await app.inject({method:"POST",url:"/v1/messages",headers:{...headers,"agmsg-team-id":other},payload:{messages:[{id:randomUUID(),envelope}]}})).statusCode).toBe(200);
      expect(await Promise.race([pending.then(()=>"unexpected hint"),pause(250).then(()=>"silent")])).toBe("silent");
      await app.inject({method:"POST",url:"/v1/messages",headers,payload:{messages:[{id:randomUUID(),envelope}]}});
      expect(await pending).toEqual(initial);
      // The LISTEN backend is scoped to this test application/schema. Losing
      // it must close the HTTP stream, rather than leave a healthy-looking SSE.
      const listener=await admin.query("SELECT pid FROM pg_stat_activity WHERE application_name=$1",[`${schema}:agent-mail-events`]);
      expect(listener.rows).toHaveLength(1);
      await admin.query("SELECT pg_terminate_backend($1)",[listener.rows[0].pid]);
      const closed=await bounded(stream.reader.read().then(result=>result.done,()=>true));
      expect(closed).toBe(true);
      const reconnected=await connect();expect(await reconnected.next()).toEqual(initial);
      await bounded(app.close());app=undefined;
    } finally {
      for(const controller of controllers) controller.abort();
      await app?.close();await pool?.end();
      if(created) await admin.query(`DROP SCHEMA ${schema} CASCADE`);
      await admin.end();
    }
  },20000);
});
