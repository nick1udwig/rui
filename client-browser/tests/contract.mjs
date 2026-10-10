import init, { Receiver } from '../wasm/rui_client_wasm.js';
import { BrowserClient } from '../dist/client.js';
const assert=(ok,message)=>{if(!ok)throw Error(message)};
const enc=new TextEncoder();
const join=(...parts)=>{const out=new Uint8Array(parts.reduce((n,p)=>n+p.length,0));let at=0;for(const p of parts){out.set(p,at);at+=p.length;}return out;};
const uint=value=>{value=BigInt(value);const out=[];do{out.push(Number(value&127n)|(value>=128n?128:0));value>>=7n;}while(value);return new Uint8Array(out);};
const scalar=(tag,v)=>join(uint(tag*8),uint(v));
const blob=(tag,v)=>{v=typeof v==='string'?enc.encode(v):v;return join(uint(tag*8+2),uint(v.length),v);};
const ids=[1,2,3,4].map(n=>new Uint8Array(16).fill(n));
const packet=(kind,body)=>join(blob(1,ids[0]),blob(2,ids[1]),blob(3,ids[3]),scalar(4,1),scalar(5,kind),blob(6,body));
function canonical(value){
  if(typeof value==='number'||typeof value==='bigint')return String(value);
  if(value instanceof Map)return canonical(Object.fromEntries([...value].map(([k,v])=>[String(k),v])));
  if(Array.isArray(value)||value instanceof Set)return [...value].map(canonical);
  if(value&&typeof value==='object')return Object.fromEntries(Object.keys(value).sort().map(k=>[k,canonical(value[k])]));
  return value;
}
export async function checkContract(){
  await init();
  const fixture=await (await fetch('/wasm/contract.json')).json();
  const payload=new Uint8Array(fixture.hex.match(/../g).map(x=>parseInt(x,16)));
  const receiver=new Receiver(...ids,32<<20,false);
  try {
    const envelope=join(scalar(1,1),blob(2,ids[0]),blob(3,ids[1]),blob(4,ids[3]),scalar(5,1),blob(6,payload));
    receiver.receiveEnvelope(ids[0],envelope);
    const state=receiver.state();
    assert(JSON.stringify(canonical(state))===JSON.stringify(canonical(fixture.state)),'complete native/Python/WASM state differs');
    assert(new Set([...state.records.values()].map(r=>Object.keys(r)[0])).size===6,'record variant omitted');
    assert(state.revision===0xffffffffffffffffn,'maximum clock rounded');
    const values=[...state.records.values()].filter(r=>'Semantic'in r).map(r=>r.Semantic.value);
    assert(values.some(v=>v?.Integer===-0x8000000000000000n),'signed integer rounded');
  } finally {receiver.close();assert(receiver.memoryUsed===0,'contract leaked');receiver.free();}
  const profiles=['mirror2d.core.v1','semantics.core.v1','content.no-raster.v1','glyph.outline-single.v1','text.clock.gtk-editable4224.v1'];
  const limits=[1<<20,32<<10,16<<20,64<<20,16<<20,128<<20,8<<20,100000,128];
  const hello=packet(2,join(scalar(1,1),...profiles.map(p=>blob(2,p)),blob(3,join(...limits.map((v,i)=>scalar(i+1,v)))),blob(4,join(scalar(1,1),scalar(2,1),blob(3,'Activate'))),blob(5,ids[2])));
  const canvas=document.createElement('canvas');document.body.append(canvas);
  let callbacks,admitted=[],credentialResolve;
  const credential=new Promise(resolve=>credentialResolve=resolve);
  const renderer={render(){},reset(){},dispose(){}};
  const client=await BrowserClient.create({url:'ws://localhost',allowInsecure:true,clientId:ids[2],credential:()=>credential,selection:'owned',canvas,renderer,transport:(url,cbs)=>{callbacks=cbs;queueMicrotask(()=>{cbs.opened();cbs.message(hello.buffer);});return {bufferedAmount:0,send(bytes){admitted.push(bytes.slice());},close(){}};}});
  const first=client.connect();
  let rejected=false;try{await client.connect()}catch{rejected=true}
  assert(rejected&&client.connectionState==='connecting','concurrent credential attempts admitted');
  credentialResolve(new Uint8Array(32).fill(7));await first;
  assert(client.connectionState==='connected','connection state missing');
  const digest=new Uint8Array(await crypto.subtle.digest('SHA-256',payload));
  let laterCalled=false;
  client.subscribe(event=>{if(event.type==='applied')client.dispose();});
  client.subscribe(event=>{if(event.type==='applied')laterCalled=true;});
  const publication=new Uint8Array(16).fill(9);
  const meta=join(blob(1,publication),scalar(3,0xffffffffffffffffn),scalar(4,payload.length),scalar(5,1),scalar(6,fixture.state.records?Object.keys(fixture.state.records).length:0),blob(7,digest));
  callbacks.message(packet(10,meta).buffer);
  callbacks.message(packet(12,join(blob(1,publication),scalar(2,0),blob(3,payload))).buffer);
  callbacks.message(packet(13,join(blob(1,publication),scalar(2,payload.length),scalar(3,1),scalar(4,Object.keys(fixture.state.records).length),blob(5,digest))).buffer);
  assert(client.connectionState==='disposed'&&!laterCalled,'event disposal did not stop delivery');
  const count=admitted.length;callbacks.opened();callbacks.message(hello.buffer);
  assert(admitted.length===count,'stale transport callback emitted bytes');
  canvas.remove();
  return {completeModel:true,maximumWidthIntegers:true,independentPythonOracle:true,concurrentConnect:true,listenerDisposal:true,generationFencing:true};
}
