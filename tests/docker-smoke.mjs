// Builds are separate: test only a newly built owner-agent image in private volumes.
// No host development database, existing Compose project, model or tool runner is used.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes,createHash} from 'node:crypto';
import {oauth} from './fixtures/pasion-auth.mjs';
const image=process.env.HAGENCY_TEST_IMAGE??'hagency-server:owner-appservice-v1';
const imageInfo=JSON.parse(execFileSync('docker',['image','inspect',image],{encoding:'utf8'}))[0];
assert.equal(imageInfo.Config.Labels?.['org.hagency.protocol'],'owner-agent-v1','Refuses an old Fleet-era image');
if(process.env.HAGENCY_TEST_IMAGE_SOURCE) assert.equal(imageInfo.Config.Labels['org.hagency.source-fingerprint'],process.env.HAGENCY_TEST_IMAGE_SOURCE,'Image differs from the reviewed build snapshot');
const reservation=createServer().listen(0,'127.0.0.1');await once(reservation,'listening');const port=reservation.address().port;await new Promise(r=>reservation.close(r));
const base=`http://127.0.0.1:${port}`,name=`localhost:${port}`,password=randomBytes(24).toString('base64url');
const dir=await mkdtemp(join(tmpdir(),'hagency-owner-compose-')),compose=join(dir,'compose.yaml'),project='hagency-owner-test-'+randomBytes(6).toString('hex');
const run=(...args)=>execFileSync('docker',['compose','--project-name',project,'-f',compose,...args],{encoding:'utf8',stdio:['ignore','pipe','pipe'],timeout:180_000});
const configDir=join(dir,'config');await mkdir(configDir,{mode:0o700});
const hostConfig=`listen = "0.0.0.0:8088"\npublic_origin = "${base}"\ndata_dir = "/app/data"\npublic_dir = "/app/resources/frontend/public"\ndatabase_url = "postgres://hagency:compose_test_only@postgres:5432/hagency"\npalpo_config = "palpo.toml"\npasion_config = "pasion.toml"\n[queue]\nevent_ttl_ms = 86400000\n`;
const palpoConfig=`server_name = "${name}"\nallow_registration = false\n[db]\nurl = "postgres://hagency:compose_test_only@postgres:5432/palpo"\npool_size = 10\n[well_known]\nclient = "${base}"\nserver = "${name}"\n[storage]\nbackend = "fs"\nroot = "/app/data/media"\n`;
const pasionConfig=`[database]\nuri = "postgres://hagency:compose_test_only@postgres:5432/pasion"\n[account]\npassword_registration_enabled = true\npassword_registration_contact_required = false\n[hagency]\nresources_dir = "/app/resources/pasion"\ndelegate_matrix_auth = true\n`;
for(const [component,text] of [['hagency',hostConfig],['palpo',palpoConfig],['pasion',pasionConfig]]) await writeFile(join(configDir,component+'.toml'),text,{mode:0o600});
await writeFile(join(dir,'init.sql'),await readFile(resolve('deploy/databases.sql'),'utf8'));
await writeFile(join(dir,'admin-password'),password,{mode:0o600});
function definition(bootstrap){return `services:\n  postgres:\n    image: postgres:18.6-alpine\n    environment:\n      POSTGRES_USER: hagency\n      POSTGRES_PASSWORD: compose_test_only\n      POSTGRES_DB: hagency\n    volumes: ["database:/var/lib/postgresql", "${dir}/init.sql:/docker-entrypoint-initdb.d/20-components.sql:ro"]\n    healthcheck:\n      test: ["CMD-SHELL", "pg_isready -U hagency -d hagency"]\n      interval: 1s\n      timeout: 3s\n      retries: 30\n  server:\n    image: ${image}\n    depends_on:\n      postgres: {condition: service_healthy}\n    ports: ["127.0.0.1:${port}:8088"]\n    volumes:\n      - "${configDir}:/app/config:ro"\n${bootstrap?`      - "${dir}/admin-password:/app/bootstrap-password:ro"\n`:''}      - "server-data:/app/data"\n    command: ${JSON.stringify(['--config','/app/config/hagency.toml',...(bootstrap?['--bootstrap-admin','admin','--bootstrap-password-file','/run/hagency/bootstrap-password']:[])])}\n    stop_grace_period: 40s\nvolumes:\n  database: {}\n  server-data: {}\n`;}
async function until(check,label,seconds=90){const deadline=Date.now()+seconds*1000;while(Date.now()<deadline){if(await check())return;await new Promise(r=>setTimeout(r,200));}throw new Error(label+' timed out');}
async function ready(){await until(async()=>{try{const r=await fetch(base+'/readyz',{signal:AbortSignal.timeout(3000)});return r.status===200&&(await r.json()).startupRoundtripConfirmed===true;}catch{return false;}},'Startup AS roundtrip',180);}
async function api(path,{method='GET',token,body,cookie,origin,csrf,status=[200]}={}){
 const headers={};if(token)headers.Authorization='Bearer '+token;if(cookie)headers.Cookie=cookie;if(origin)headers.Origin=origin;if(csrf)headers['x-csrf-token']=csrf;if(body!==undefined)headers['content-type']='application/json';
 const r=await fetch(base+path,{method,headers,body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(15_000)});const v=await r.json();
 assert.ok(status.includes(r.status),`${method} ${path}: HTTP ${r.status}, ${v.code??v.errcode??'unexpected response'}`);return {value:v,response:r};
}
const sql=(db,statement)=>run('exec','-T','postgres','psql','-U','hagency','-d',db,'-Atc',statement).trim();
const scalar=s=>"'"+s.replaceAll("'","''")+"'";
async function browserLogin(username,secret){
 const {value,response}=await api('/auth/api/v1/auth/login',{method:'POST',origin:base,body:{username,password:secret}});assert.equal(value.status,'success');
 return response.headers.getSetCookie().map(c=>c.split(';')[0]).join('; ');
}
async function adminLogin(){
 await until(()=>sql('palpo',`SELECT is_admin FROM users WHERE id=${scalar('@admin:'+name)}`)==='t','Administrator Matrix provisioning');
 const pasionCookie=await browserLogin('admin',password);const upstream=await oauth(base,pasionCookie,true);
 const {value,response}=await api('/api/login/token',{method:'POST',origin:base,token:upstream.access_token,body:{}});assert.equal(value.isAdmin,true);assert.ok(value.csrf);
 const cookie=response.headers.getSetCookie().map(c=>c.split(';')[0]).join('; ');
 assert.equal((await api('/api/session',{origin:base,cookie})).value.userId,'@admin:'+name);
 assert.equal((await fetch(base+'/_palpo/admin/v1/server_version',{headers:{Authorization:'Bearer '+upstream.access_token}})).status,200);
 return {cookie,csrf:value.csrf};
}
async function nativeOAuth(cookie){
 const {value:client}=await api('/auth/oauth2/registration',{method:'POST',body:{client_name:'Isolated Docker owner Agent PKCE smoke',client_uri:'https://github.com/chrislearn/hagency-client',application_type:'native',token_endpoint_auth_method:'none',grant_types:['authorization_code','refresh_token'],response_types:['code'],redirect_uris:['http://127.0.0.1/console/server-login/callback']},status:[200,201]});
 const verifier=randomBytes(48).toString('base64url'),state=randomBytes(24).toString('hex'),redirect=`http://127.0.0.1:${port}/console/server-login/callback`;
 const query=new URLSearchParams({client_id:client.client_id,response_type:'code',redirect_uri:redirect,state,scope:'urn:matrix:client:api:* urn:matrix:client:device:DockerOwnerSmoke',code_challenge:createHash('sha256').update(verifier).digest('base64url'),code_challenge_method:'S256'});
 const auth=await fetch(base+'/auth/authorize?'+query,{headers:{Cookie:cookie},redirect:'manual',signal:AbortSignal.timeout(15_000)});assert.ok([302,303].includes(auth.status));let next=new URL(auth.headers.get('location'),base);
 if(!next.pathname.endsWith('/console/server-login/callback')){
  const grant=next.pathname.split('/').pop();const consent=await api('/auth/api/v1/oauth2/consent/'+grant,{method:'POST',cookie,origin:base,body:{action:'consent'}});assert.equal(consent.value.status,'success');next=new URL(consent.value.redirect_url);
 }
 assert.equal(next.searchParams.get('state'),state);assert.ok(next.searchParams.get('code'));
 const token=await fetch(base+'/auth/oauth2/token',{method:'POST',headers:{'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({grant_type:'authorization_code',client_id:client.client_id,redirect_uri:redirect,code:next.searchParams.get('code'),code_verifier:verifier}),signal:AbortSignal.timeout(15_000)});assert.equal(token.status,200);const result=await token.json();assert.ok(result.access_token);return result.access_token;
}
let memberPassword=randomBytes(24).toString('base64url');
async function memberLogin(register){
 if(register){const {value,response}=await api('/auth/api/v1/auth/register',{method:'POST',origin:base,body:{username:'member',password:memberPassword,password_confirm:memberPassword}});assert.equal(value.status,'success');assert.ok(value.id);
  let registrationCookie=response.headers.getSetCookie().map(c=>c.split(';')[0]).join('; ');
  const display=await api('/auth/api/v1/auth/register/'+value.id+'/display-name',{method:'POST',origin:base,cookie:registrationCookie,body:{display_name:'Ordinary owner'}});
  registrationCookie=display.response.headers.getSetCookie().map(c=>c.split(';')[0]).join('; ')||registrationCookie;
  const finish=await api('/auth/api/v1/auth/register/'+value.id+'/finish',{method:'POST',origin:base,cookie:registrationCookie,body:{}});assert.equal(finish.value.status,'success');
 }
 await until(()=>sql('palpo',`SELECT is_admin FROM users WHERE id=${scalar('@member:'+name)}`)==='f','Ordinary member Matrix provisioning');
 const cookie=await browserLogin('member',memberPassword),upstream=await nativeOAuth(cookie);
 const {value:session}=await api('/api/hagency/v1/sessions/pasion',{method:'POST',body:{accessToken:upstream}});assert.equal(session.mxid,'@member:'+name);assert.ok(session.token);assert.ok(session.validUntilMs>Date.now());
 assert.equal((await fetch(base+'/_palpo/admin/v1/server_version',{headers:{Authorization:'Bearer '+upstream}})).status,403,'Member grant cannot administer Palpo');
 return {upstream,session};
}
let context;
async function exerciseOwner(member,create){
 const renew=async()=>api('/api/hagency/v1/sessions/current/renew',{method:'POST',token:member.session.token,body:{accessToken:member.upstream}});
 const owner=async(path,method='GET',body)=>{await renew();return api('/api/hagency/v1/'+path,{method,token:member.session.token,body,status:[200,202]});};
 const matrix=async(path,method='GET',body)=>api('/_matrix/client/v3/'+path,{method,token:member.upstream,body});
 if(create){
  const discovery=(await api('/api/hagency/v1/discovery')).value;assert.equal(discovery.protocolVersion,1);assert.equal(discovery.serviceMxid,'@_hagency_service:'+name);
  const space=(await matrix('createRoom','POST',{preset:'private_chat',name:'Docker owner project',creation_content:{type:'m.space'}})).value.room_id;
  const room=(await matrix('createRoom','POST',{preset:'private_chat',name:'Private discussion',invite:[discovery.serviceMxid]})).value.room_id;
  await matrix('rooms/'+encodeURIComponent(space)+'/state/m.space.child/'+encodeURIComponent(room),'PUT',{via:[name]});
  const project=(await owner('projects/adopt','POST',{spaceId:space})).value.project;
  await owner('projects/'+project.id+'/rooms/adopt','POST',{roomId:room});
  const created=await owner('agents','POST',{projectId:project.id,roomId:room,displayName:'Owner Codex puppet',idempotencyKey:'docker-owner-agent'});assert.equal(created.response.status,202);assert.equal(created.value.commandState,'pending');
  context={space,room,project,agent:created.value.creation.agent,binding:created.value.creation.binding};
 }
 const {agent,binding,room,project}=context;
 assert.equal(agent.ownerUserId,member.session.userId);
 await until(async()=>{const current=(await owner('bindings/'+binding.id)).value.binding;assert.equal(current.agentId,agent.id);return current.state==='active';},'Durable puppet provisioning');
 const current=(await owner('agents/'+agent.id)).value.agent;assert.equal(current.ownerUserId,member.session.userId);assert.equal(current.puppetMxid,agent.puppetMxid);
 assert.ok((await owner('projects/'+project.id+'/rooms')).value.rooms.some(r=>r.roomId===room));
 const roster=(await owner('projects/'+project.id+'/rooms/'+encodeURIComponent(room)+'/agents')).value.agents;assert.ok(roster.some(a=>a.agentId===agent.id&&a.ownerMxid===member.session.mxid));
 const device=(await owner('devices','POST',{installationId:'docker-owner-installation',name:'Docker protocol fixture'})).value;
 const deviceCall=(path,body,status=[200])=>api('/api/hagency/v1/execution/'+path,{method:'POST',token:device.token,body,status});
 const history=async()=>{
  const page=(await deviceCall('history',{agentId:agent.id})).value.history;
  const rows=[...page.executions];let cursor=page.nextCursor;
  while(cursor){const next=(await deviceCall('history',{agentId:agent.id,cursor,snapshot:page.snapshot})).value.history;assert.deepEqual(next.snapshot,page.snapshot);rows.push(...next.executions);cursor=next.nextCursor;}
  assert.equal(rows.length,page.snapshot.count);const digest=createHash('sha256').update('hagency-started-executions-v1\n');for(const row of [...rows].sort((a,b)=>a.dispatchId<b.dispatchId?-1:a.dispatchId>b.dispatchId?1:0))digest.update(JSON.stringify([row.dispatchId,row.executionId])+'\n');assert.equal(digest.digest('hex'),page.snapshot.digest);
  for(const known of context.executionProofs||[]){const row=rows.find(r=>r.dispatchId===known.dispatchId);assert.ok(row,'Started execution remains discoverable after restart');assert.equal(row.executionId,known.executionId);assert.equal(row.immutableDigest,known.immutableDigest);}
  return {snapshot:page.snapshot,rows};
 };
 const before=await history();assert.equal(before.snapshot.count,create?0:1);
 const denied=await deviceCall('leases/acquire',{agentId:agent.id,ttlMs:10000,takeover:!create,historySnapshot:{...before.snapshot,digest:'f'.repeat(64)}},[409]);assert.equal(denied.value.code,'execution_history_changed');
 const lease=(await deviceCall('leases/acquire',{agentId:agent.id,ttlMs:10000,takeover:!create,historySnapshot:before.snapshot})).value.lease;
 const reference={agentId:agent.id,epoch:lease.epoch};
 const event=(await matrix('rooms/'+encodeURIComponent(room)+'/send/m.room.message/docker_request_'+(create?'fresh':'restart'),'PUT',{msgtype:'m.text',body:'Protocol fixture: no model invocation', 'm.mentions':{user_ids:[agent.puppetMxid]}})).value.event_id;
 const heartbeat=async()=>deviceCall('leases/renew',{lease:reference,ttlMs:10000});
 let dispatch;await until(async()=>{await renew();await heartbeat();const values=(await deviceCall('events/poll',{lease:reference,bindingId:binding.id,limit:10})).value.events;dispatch=values.find(e=>e.eventId===event);return !!dispatch;},'Durable Appservice routing',20);
 assert.equal(dispatch.roomId,room);assert.equal(dispatch.requesterMxid,member.session.mxid);
 await deviceCall('events/ack',{lease:reference,dispatchId:dispatch.id});
 const executionId='docker_exec_'+(create?'fresh':'restart');
 assert.equal((await deviceCall('events/start',{lease:reference,dispatchId:dispatch.id,executionId})).value.execution.newlyStarted,true);
 assert.equal((await deviceCall('events/start',{lease:reference,dispatchId:dispatch.id,executionId})).value.execution.newlyStarted,false);
 const after=await history();assert.equal(after.snapshot.count,before.snapshot.count+1);const stale=await deviceCall('leases/acquire',{agentId:agent.id,ttlMs:10000,takeover:true,historySnapshot:before.snapshot},[409]);assert.equal(stale.value.code,'execution_history_changed');await heartbeat();const witness=after.rows.find(row=>row.dispatchId===dispatch.id);assert.ok(witness);assert.equal(witness.executionId,executionId);assert.equal(witness.bindingId,binding.id);assert.equal(witness.dispatchEpoch,reference.epoch);assert.match(witness.immutableDigest,/^[a-f0-9]{64}$/);context.executionProofs=[...(context.executionProofs||[]),{dispatchId:dispatch.id,executionId,immutableDigest:witness.immutableDigest}];
 const text='Docker known reply '+(create?'fresh':'restart')+'; no model was invoked';
 const reply=(await deviceCall('replies',{lease:reference,reply:{dispatchId:dispatch.id,executionId,body:text}})).value.reply;assert.equal(reply.body,text);
 await until(async()=>{await renew();await heartbeat();return sql('hagency',`SELECT state FROM hagency_agent_v1.reply_outbox WHERE id=${scalar(reply.id)}`)==='sent';},'Puppet Matrix delivery',20);
 const matrixEvent=sql('hagency',`SELECT matrix_event_id FROM hagency_agent_v1.reply_outbox WHERE id=${scalar(reply.id)}`);assert.ok(matrixEvent.startsWith('$'));
 const sent=(await matrix('rooms/'+encodeURIComponent(room)+'/event/'+encodeURIComponent(matrixEvent))).value;
 assert.equal(sent.sender,agent.puppetMxid);assert.equal(sent.content.body,text);assert.equal(sent.content['m.relates_to'].event_id,dispatch.threadRoot);
 await deviceCall('leases/release',{lease:reference});
 return device;
}
function persistedIdentity(){return run('exec','-T','server','sha256sum','/app/data/agent-appservice.json','/app/data/matrix-signing-key.json','/app/data/pasion-secrets.json').trim();}
try{
 await writeFile(compose,definition(true));run('up','-d');await ready();
 const serverContainer=run('ps','-q','server').trim();
 await until(()=>execFileSync('docker',['inspect','--format','{{.State.Health.Status}}',serverContainer],{encoding:'utf8'}).trim()==='healthy','Container readiness healthcheck');
 console.log('PASS fresh mandatory appservice roundtrip and Docker healthy');
 const admin=await adminLogin();
 const tableNames=(db,schema)=>sql(db,`SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname=${scalar(schema)} ORDER BY tablename`).split('\n');
 assert.ok(tableNames('hagency','hagency_agent_v1').includes('owner_events'));assert.ok(tableNames('hagency','hagency_agent_v1').includes('reply_outbox'));assert.ok(!tableNames('hagency','public').includes('hagency_admin_state'));assert.ok(tableNames('palpo','public').includes('users'));assert.ok(tableNames('pasion','public').length>1);
 assert.equal(sql('palpo',"SELECT count(*) FROM appservice_registrations WHERE id='hagency_agents_v1'"),'1');
 assert.ok(Number(sql('hagency',"SELECT count(*) FROM hagency_agent_v1.readiness_receipts"))>0,'startup readiness requires a persisted real AS receipt');
 const homepage=await(await fetch(base)).text();assert.match(homepage,/<title>Hagency<\/title>/);assert.ok(homepage.includes('href="/hagency/home"'));
 const frontend=await(await fetch(base+'/hagency/home')).text();assert.match(frontend,/<title>Hagency Server<\/title>/);assert.notEqual(homepage,frontend);assert.equal(await(await fetch(base+'/hagency/agents')).text(),frontend);
 const frontScript=frontend.match(/src="([^"]+\.js)"/)[1],module=await(await fetch(new URL(frontScript,base))).text(),frontWasm=module.match(/module_or_path:\s*"([^"]+\.wasm)"/)[1];
 const wasmResponse=await fetch(new URL(frontWasm,base));assert.equal(wasmResponse.status,200);assert.match(wasmResponse.headers.get('content-type'),/application\/wasm/);assert.equal(Buffer.from(await wasmResponse.arrayBuffer()).subarray(0,4).toString('hex'),'0061736d');
 const runtime=await(await fetch(base+'/config.json')).json();assert.equal(runtime.pasion_enabled,true);assert.equal(runtime.oauth_enabled,true);
 assert.equal((await fetch(base+'/_matrix/client/versions')).status,200);assert.equal((await(await fetch(base+'/auth/.well-known/openid-configuration')).json()).issuer,base+'/auth/');assert.equal((await fetch(base+'/auth/login')).status,200);
 const keys=await(await fetch(base+'/auth/oauth2/keys.json')).json();assert.ok(keys.keys.length);
 for(const path of ['/api/fleets','/api/approvals','/api/accounts','/api/registrations'])assert.equal((await fetch(base+path,{headers:{Cookie:admin.cookie}})).status,404);
 const facts=run('exec','-T','server','sh','-c',"awk '/^Uid:/ {print $2}' /proc/1/status; stat -c %a /run/hagency/config/*.toml /run/hagency/config /app/data /app/data/agent-appservice.json /app/data/pasion-secrets.json").trim().split('\n');assert.deepEqual(facts,['10001','600','600','600','700','700','600','600']);
 console.log('PASS three databases, administrator auth, frontend, removed Fleet APIs and private non-root runtime');
 const member=await memberLogin(true);console.log('PASS ordinary member native Pasion DCR/PKCE and non-admin privileges');
 const firstDevice=await exerciseOwner(member,true),identity=persistedIdentity();console.log('PASS ordinary owner Space/Room/Agent and actual puppet Matrix reply');
 run('stop','server');await writeFile(compose,definition(false));run('up','-d','server');await ready();assert.equal((await fetch(base+'/api/session',{headers:{Cookie:admin.cookie,Origin:base}})).status,401,'In-memory browser session is not revived after restart');await adminLogin();
 console.log('PASS restarted mandatory appservice ready and stale browser session rejection');
 assert.equal(persistedIdentity(),identity);assert.deepEqual(await(await fetch(base+'/auth/oauth2/keys.json')).json(),keys);assert.equal(sql('palpo',"SELECT count(*) FROM appservice_registrations WHERE id='hagency_agents_v1'"),'1');
 const secondMember=await memberLogin(false);assert.equal(secondMember.session.userId,member.session.userId);const secondDevice=await exerciseOwner(secondMember,false);assert.equal(secondDevice.deviceId,firstDevice.deviceId);assert.ok(secondDevice.generation>firstDevice.generation);
 console.log('PASS new owner-agent image',imageInfo.Id.slice(0,19),': isolated Compose/3DB, mandatory AS roundtrip, native Pasion PKCE, ordinary member Space+Room+Agent, durable puppet replies, non-root/private configs, removed Fleet APIs and immutable identity across restart; no model invoked');
}catch(error){console.error(run('logs','--no-color','--tail','35','server'));throw error;}finally{try{run('down','--volumes','--remove-orphans');}finally{await rm(dir,{recursive:true,force:true});}}
