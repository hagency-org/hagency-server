// Three EMPTY dedicated databases; starts only an isolated test process.
import assert from 'node:assert/strict';
import {spawn, execFileSync} from 'node:child_process';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes} from 'node:crypto';
import {oauth as authorize} from './fixtures/pasion-auth.mjs';
const oauth=(cookie,admin=false)=>authorize(base,cookie,admin);
import {mkdtemp, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
const admin = process.env.HAGENCY_TEST_DATABASE_URL, matrix = process.env.PALPO_TEST_DATABASE_URL, pasion = process.env.PASION_TEST_DATABASE_URL;
assert.ok(admin && matrix && pasion, 'Set EMPTY HAGENCY_TEST_DATABASE_URL, PALPO_TEST_DATABASE_URL and PASION_TEST_DATABASE_URL');
for (const database of [admin,matrix,pasion]) assert.equal(execFileSync('psql',[database,'-Atc',"SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema')"],{encoding:'utf8'}).trim(),'0','Refuses a populated database');
const reserve=createServer().listen(0,'127.0.0.1');await once(reserve,'listening');const port=reserve.address().port;await new Promise(r=>reserve.close(r));
const dir=await mkdtemp(join(tmpdir(),'hagency-pasion-')),base=`http://127.0.0.1:${port}`;
const config=join(dir,'hagency.toml'),binary=process.env.HAGENCY_BINARY??resolve('target/debug/hagency-server');
const resources=resolve(process.env.PASION_TEST_RESOURCES??'resources/pasion');
let child, logs='',cookie='';
const pause=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn){for(let i=0;i<180;i++){if(child?.exitCode!==null&&child?.exitCode!==undefined)throw Error(logs);try{if(await fn())return;}catch(e){if(i===179)throw e;}await pause(500);}throw Error('Timed out: '+logs);}
async function configure(delegate=false){
 await writeFile(config,`listen = "127.0.0.1:${port}"\npublic_origin = "${base}"\ndata_dir = ${JSON.stringify(join(dir,"data"))}\ndatabase_url = ${JSON.stringify(admin)}\npalpo_config = "palpo.toml"\npasion_config = "pasion.toml"\n`,{mode:0o600});
 await writeFile(join(dir,'palpo.toml'),`server_name = "localhost:${port}"\nallow_registration = false\n[db]\nurl = ${JSON.stringify(matrix)}\npool_size = 10\n[well_known]\nclient = "${base}"\nserver = "localhost:${port}"\n[storage]\nbackend = "fs"\nroot = "data/media"\n`,{mode:0o600});
 await writeFile(join(dir,'pasion.toml'),`[database]\nuri = ${JSON.stringify(pasion)}\nmax_connections = 6\n[hagency]\nresources_dir = ${JSON.stringify(resources)}\ndelegate_matrix_auth = ${delegate}\n[account]\npassword_registration_enabled = true\npassword_registration_contact_required = false\n`,{mode:0o600});
}
async function start(args=[]){child=spawn(binary,['--config',config,...args],{stdio:['ignore','pipe','pipe'],env:{...process.env,RUST_LOG:'warn'}});child.stdout.on('data',b=>logs+=b);child.stderr.on('data',b=>logs+=b);await until(async()=> (await fetch(base+'/auth/healthz')).status===200);}
async function stop(){if(child&&child.exitCode===null){const done=once(child,'exit');child.kill('SIGTERM');const result=await Promise.race([done,pause(35000).then(()=>{throw Error('Shutdown timed out')})]);assert.equal(result[0],0,logs);}child=undefined;}
async function api(path,body){const r=await fetch(base+'/auth/api/v1'+path,{method:body?'POST':'GET',headers:{'content-type':'application/json',...(cookie?{cookie}:{})},body:body?JSON.stringify(body):undefined});const set=r.headers.getSetCookie();if(set.length)cookie=set.map(s=>s.split(';')[0]).join('; ');const data=await r.json();assert.equal(r.status,200,JSON.stringify(data));return data;}
async function adminApi(token,path,body) {
 const response=await fetch(base+'/auth/api/admin/v1'+path,{method:body?'PATCH':'GET',headers:{Authorization:'Bearer '+token,'content-type':'application/json',Origin:base},body:body?JSON.stringify(body):undefined});
 const value=await response.json();assert.equal(response.status,200,JSON.stringify(value));return value;
}

try{
 await configure();await start();
 const tables=(url)=>execFileSync('psql',[url,'-Atc',"SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname='public' ORDER BY tablename"],{encoding:'utf8'}).trim().split('\n');
 assert.deepEqual(tables(admin),['hagency_admin_state']);assert.ok(tables(matrix).includes('users'));assert.ok(!tables(matrix).includes('hagency_admin_state'));assert.ok(tables(pasion).length>1);assert.ok(!tables(pasion).includes('hagency_admin_state'));
 assert.equal((await fetch(base+'/')).status,200);assert.equal((await fetch(base+'/_matrix/client/versions')).status,200);
 const discovery=await (await fetch(base+'/auth/.well-known/openid-configuration')).json();assert.equal(discovery.issuer,base+'/auth/');assert.equal(discovery.authorization_endpoint,base+'/auth/authorize');assert.equal(discovery.token_endpoint,base+'/auth/oauth2/token');
 for(const path of ['/api-doc/openapi.json','/api-doc/admin/openapi.json']) { const spec=await(await fetch(base+'/auth'+path)).json();assert.equal(spec.servers[0].url,'/auth'); }
 const alias=await fetch(base+'/auth/account/',{redirect:'manual'});assert.equal(alias.status,302);assert.equal(alias.headers.get('location'),'/auth/');
 const jwks=await(await fetch(base+'/auth/oauth2/keys.json')).json();assert.ok(jwks.keys.length);
 const html=await(await fetch(base+'/auth/login')).text();assert.ok(html.includes('/auth/api/v1'));const script=html.match(/src="([^"]+\.js)"/)[1].replace(/&#x([0-9a-f]+);/gi,(_,h)=>String.fromCodePoint(parseInt(h,16))).replace(/&#([0-9]+);/g,(_,n)=>String.fromCodePoint(Number(n)));assert.ok(script.startsWith('/auth/assets/'));
 const js=await(await fetch(base+script)).text();const wasm=js.match(/module_or_path:"([^"]+\.wasm)"/)[1];assert.ok(wasm.startsWith('/auth/assets/'));const wr=await fetch(base+wasm);assert.equal(wr.status,200);assert.match(wr.headers.get('content-type'),/application\/wasm/);
 assert.notEqual((await fetch(base+'/oauth2/keys.json')).status,200,'Pasion must not claim root routes');
 const internal=await fetch(base+'/auth/api/internal/matrix/password-login',{method:'POST',headers:{'content-type':'application/json'},body:'{}'});assert.equal(internal.status,401);
 const providers=await api('/auth/providers');assert.equal(providers.password_registration_enabled,true);
 const password='Test account! Matrix prefix 93614';const registration=await api('/auth/register',{username:'prefixuser',password,password_confirm:password});assert.equal(registration.status,'success',JSON.stringify(registration));assert.ok(registration.id);
 const display=await api('/auth/register/'+registration.id+'/display-name',{display_name:'Prefix User'});assert.equal(display.status,'success',JSON.stringify(display));
 const finished=await api('/auth/register/'+registration.id+'/finish',{});assert.equal(finished.status,'success',JSON.stringify(finished));
 const login=await api('/auth/login',{username:'prefixuser',password});assert.equal(login.status,'success',JSON.stringify(login));assert.equal(login.viewer.username,'prefixuser');
 assert.ok(cookie,'Pasion browser login must set its cookie');const viewer=await api('/viewer');assert.ok(JSON.stringify(viewer).includes('prefixuser'));
 await until(async()=>execFileSync('psql',[matrix,'-Atc',`SELECT count(*) FROM public.users WHERE id='@prefixuser:localhost:${port}'`],{encoding:'utf8'}).trim()==='1');
 const nativeRuntime=await(await fetch(base+'/config.json')).json();assert.equal(nativeRuntime.oauth_enabled,false);assert.equal(nativeRuntime.pasion_enabled,true);
 const secrets=await readFile(join(dir,'data/pasion-secrets.json'),'utf8');
 await stop();await configure(true);
 const adminPassword=randomBytes(32).toString('base64url'),adminFile=join(dir,'admin-password');
 await writeFile(adminFile,adminPassword,{mode:0o600});
 await start(['--bootstrap-admin','serveradmin','--bootstrap-password-file',adminFile]);assert.equal(await readFile(join(dir,'data/pasion-secrets.json'),'utf8'),secrets);
 assert.deepEqual(await(await fetch(base+'/auth/oauth2/keys.json')).json(),jwks);
 const authentication=await(await fetch(base+'/.well-known/matrix/client')).json();assert.equal(authentication['m.authentication'].issuer,base+'/auth/');
 const matrixLogin=await fetch(base+'/_matrix/client/v3/login',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({type:'m.login.password',identifier:{type:'m.id.user',user:'prefixuser'},password})});const token=await matrixLogin.json();assert.equal(matrixLogin.status,200,JSON.stringify(token));assert.ok(token.access_token);
 const me=await fetch(base+'/_matrix/client/v3/account/whoami',{headers:{authorization:'Bearer '+token.access_token}});assert.equal(me.status,200,await me.clone().text());assert.equal((await me.json()).user_id,`@prefixuser:localhost:${port}`);
 const runtime=await(await fetch(base+'/config.json')).json();assert.equal(runtime.oauth_enabled,true);assert.equal(runtime.oauth_client_id,'01KMQPADM1N000000000000000');
 const bridge=await fetch(base+'/api/login/token',{method:'POST',headers:{'content-type':'application/json',Origin:base,authorization:'Bearer '+token.access_token},body:'{}'});assert.equal(bridge.status,200,await bridge.clone().text());assert.equal((await bridge.json()).isAdmin,false);

 // The bootstrap uses Pasion, and its normal worker creates the Matrix identity.
 await until(async()=>execFileSync('psql',[matrix,'-Atc',`SELECT count(*) FROM users WHERE id='@serveradmin:localhost:${port}' AND is_admin`],{encoding:'utf8'}).trim()==='1');
 const memberCookie=cookie;
 cookie='';const adminLogin=await api('/auth/login',{username:'serveradmin',password:adminPassword});assert.equal(adminLogin.status,'success');
 const adminCookie=cookie;
 const limited=await oauth(adminCookie);assert.ok(limited.access_token);
 assert.equal((await fetch(base+'/_palpo/admin/v1/server_version',{headers:{Authorization:'Bearer '+limited.access_token}})).status,403,'A Pasion administrator using a member-only token cannot access Matrix administration');
 const limitedBridge=await fetch(base+'/api/login/token',{method:'POST',headers:{Origin:base,Authorization:'Bearer '+limited.access_token,'content-type':'application/json'},body:'{}'});assert.equal(limitedBridge.status,200);const limitedSession=limitedBridge.headers.get('set-cookie').split(';')[0];assert.equal((await limitedBridge.json()).isAdmin,false);
 const administrative=await oauth(adminCookie,true);assert.ok(administrative.access_token);assert.ok(administrative.scope.includes('urn:pasion:admin'));
 assert.equal((await fetch(base+'/_palpo/admin/v1/server_version',{headers:{Authorization:'Bearer '+administrative.access_token}})).status,200);
 const adminBridge=await fetch(base+'/api/login/token',{method:'POST',headers:{Origin:base,Cookie:limitedSession,Authorization:'Bearer '+administrative.access_token,'content-type':'application/json'},body:'{}'});assert.equal(adminBridge.status,200);assert.equal((await adminBridge.json()).isAdmin,true);
 assert.equal((await fetch(base+'/_matrix/client/v3/account/whoami',{headers:{Authorization:'Bearer '+limited.access_token}})).status,401,'The host revokes the preliminary grant after successful authorization handover');
 assert.equal((await fetch(base+'/_matrix/client/v3/account/whoami',{headers:{Authorization:'Bearer '+administrative.access_token}})).status,200,'Replacement authorization keeps its Matrix device');
 const prefixId=encodeURIComponent(`@prefixuser:localhost:${port}`);
 const adminHeaders={Authorization:'Bearer '+administrative.access_token,'content-type':'application/json'};
 const profile=await fetch(base+'/_palpo/admin/v2/users/'+prefixId,{method:'PUT',headers:adminHeaders,body:JSON.stringify({displayname:'Updated through Matrix'})});assert.equal(profile.status,200,'The admin scope guard preserves cached JSON for profile updates');
 const blockedPassword=await fetch(base+'/_palpo/admin/v2/users/'+prefixId,{method:'PUT',headers:adminHeaders,body:JSON.stringify({password:'A different pass! 957235'})});assert.equal(blockedPassword.status,403,'Native password writes cannot bypass Pasion');
 const blockedCreate=await fetch(base+'/_palpo/admin/v2/users/'+encodeURIComponent(`@nativebypass:localhost:${port}`),{method:'PUT',headers:adminHeaders,body:JSON.stringify({password:'A different pass! 957235'})});assert.equal(blockedCreate.status,403);
 assert.equal(execFileSync('psql',[matrix,'-Atc',`SELECT count(*) FROM users WHERE id='@nativebypass:localhost:${port}'`],{encoding:'utf8'}).trim(),'0');
 const users=await adminApi(administrative.access_token,'/users?filter[username]=prefixuser');const prefix=users.data.find(u=>u.attributes.username==='prefixuser');assert.ok(prefix);
 const forbidden=await oauth(memberCookie,true);assert.equal(forbidden.denied,true,'Ordinary account cannot authorize admin scopes');
 await adminApi(administrative.access_token,'/users/'+prefix.id,{admin:true});
 await until(async()=>execFileSync('psql',[matrix,'-Atc',`SELECT is_admin FROM users WHERE id='@prefixuser:localhost:${port}'`],{encoding:'utf8'}).trim()==='t');
 const promoted=await oauth(memberCookie,true);assert.ok(promoted.access_token);
 const promotedBridge=await fetch(base+'/api/login/token',{method:'POST',headers:{Origin:base,Authorization:'Bearer '+promoted.access_token,'content-type':'application/json'},body:'{}'});assert.equal(promotedBridge.status,200);const promotedSession=promotedBridge.headers.get('set-cookie').split(';')[0];assert.equal((await promotedBridge.json()).isAdmin,true);
 await adminApi(administrative.access_token,'/users/'+prefix.id,{admin:false});
 assert.equal((await fetch(base+'/_palpo/admin/v1/server_version',{headers:{Authorization:'Bearer '+promoted.access_token}})).status,403,'Role revocation blocks the next call with an existing OAuth token');
 assert.equal((await fetch(base+'/api/fleets',{headers:{cookie:promotedSession}})).status,404,'Removed Fleet endpoint stays unavailable with an existing browser cookie');
 assert.equal(execFileSync('psql',[matrix,'-Atc',`SELECT is_admin FROM users WHERE id='@prefixuser:localhost:${port}'`],{encoding:'utf8'}).trim(),'f');
 const oldRegistration=await fetch(base+'/account-request',{redirect:'manual'});assert.equal(oldRegistration.status,302);assert.equal(oldRegistration.headers.get('location'),'/auth/register');
 console.log('PASS unified Pasion accounts: first administrator bootstrap, real PKCE consent, member scope denial, Matrix role synchronization and immediate revocation across Matrix/Hagency');
 if(process.env.HAGENCY_BROWSER_HOLD) {
    await writeFile(process.env.HAGENCY_BROWSER_HOLD,JSON.stringify({base,username:'prefixuser',password,adminUsername:'serveradmin',adminPassword}),{mode:0o600});
    console.log('Browser verification server ready:',base);
    const end=Date.now()+1200000;while(Date.now()<end) {try{await readFile(process.env.HAGENCY_BROWSER_HOLD+'.stop');break;}catch{}await pause(500);}
 }
 await stop();
 console.log('PASS Pasion: three component config files and isolated databases, same listener, prefixed discovery/SPA/JS/WASM/API, browser registration/login, delegated Matrix password/token introspection, stable keys on restart, graceful shutdown');
 if(process.env.HAGENCY_KEEP_TEST_CONFIG)await writeFile(process.env.HAGENCY_KEEP_TEST_CONFIG,await readFile(config),{mode:0o600});
}catch(error){console.error(logs.slice(-10000));throw error;}finally{await stop();if(!process.env.HAGENCY_KEEP_TEST_CONFIG)await rm(dir,{recursive:true,force:true});}
