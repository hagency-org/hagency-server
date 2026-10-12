import assert from 'node:assert/strict';
import {randomBytes,createHash} from 'node:crypto';
// Real PKCE authorization/consent/token exchange, using Pasion browser cookies.
export async function oauth(base,browserCookie, administrative=false) {
 const verifier=randomBytes(48).toString('base64url'), state=randomBytes(24).toString('hex');
 const client='01KMQPADM1N000000000000000',redirect=base+'/oauth/callback';
 const scope=`urn:matrix:org.matrix.msc2967.client:api:* urn:matrix:org.matrix.msc2967.client:device:TEST${randomBytes(6).toString('hex')}${administrative?' urn:palpo:admin:* urn:pasion:admin':''}`;
 const query=new URLSearchParams({client_id:client,response_type:'code',redirect_uri:redirect,state,scope,code_challenge:createHash('sha256').update(verifier).digest('base64url'),code_challenge_method:'S256'});
 const auth=await fetch(base+'/auth/authorize?'+query,{headers:{cookie:browserCookie},redirect:'manual'});
 assert.ok([302,303].includes(auth.status),await auth.clone().text());
 let next=new URL(auth.headers.get('location'),base);
 if(!next.pathname.endsWith('/oauth/callback')) {
  const grant=next.pathname.split('/').pop();
  const screen=await(await fetch(base+'/auth/api/v1/oauth2/consent/'+grant,{headers:{cookie:browserCookie}})).json();
  const consent=await fetch(base+'/auth/api/v1/oauth2/consent/'+grant,{method:'POST',headers:{cookie:browserCookie,Origin:base,'content-type':'application/json'},body:JSON.stringify({action:'consent'})});
  const data=await consent.json();
  if(screen.admin_required===true && [400,403].includes(consent.status)) return {denied:true};assert.equal(consent.status,200,JSON.stringify(data));
  assert.equal(data.status,'success',JSON.stringify(data));next=new URL(data.redirect_url);
 }
 assert.equal(next.searchParams.get('state'),state);assert.ok(next.searchParams.get('code'));
 const token=await fetch(base+'/auth/oauth2/token',{method:'POST',headers:{'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({grant_type:'authorization_code',client_id:client,redirect_uri:redirect,code:next.searchParams.get('code'),code_verifier:verifier})});
 const value=await token.json();assert.equal(token.status,200,JSON.stringify(value));return value;
}
