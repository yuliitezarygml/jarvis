// Native HTTP integration and CPU benchmark. Node is a test client only.
import fs from 'node:fs/promises';
import {performance as perf} from 'node:perf_hooks';
import assert from 'node:assert/strict';
import http from 'node:http';
const base=process.env.TTS_URL??'http://127.0.0.1:8181';
const auth=process.env.JARVIS_BEARER_TOKEN?{Authorization:'Bearer '+process.env.JARVIS_BEARER_TOKEN}:{};
const corpus=JSON.parse(await fs.readFile('audit/corpus.json'));
const payload=(data,extra={})=>({method:'POST',headers:{...auth,'Content-Type':'application/json',...extra},body:JSON.stringify(data),signal:AbortSignal.timeout(125000)});
const file=process.argv[2]??'audit/native.json';
const results={url:base,threads:process.env.JARVIS_THREADS,at:new Date().toISOString(),runs:[]};
const persist=()=>fs.writeFile(file,JSON.stringify(results,null,2));
const stats=async()=>{if(!process.env.TTS_PID)return {};const pid=process.env.TTS_PID;const stat=(await fs.readFile('/proc/'+pid+'/stat','utf8')).split(' ');const status=await fs.readFile('/proc/'+pid+'/status','utf8');return {ticks:Number(stat[13])+Number(stat[14]),rssPeakKiB:Number(status.match(/VmHWM:\s+(\d+)/)[1])}};
function analyze(pcm){
 assert.ok(pcm.length>4800 && pcm.length%2===0);
 let sum=0,peak=0;for(let i=0;i<pcm.length;i+=2){const v=pcm.readInt16LE(i)/32768;sum+=v*v;peak=Math.max(peak,Math.abs(v));}
 const rms=Math.sqrt(sum/(pcm.length/2));assert.ok(rms>0.001&&peak<=1);return {seconds:pcm.length/48000,rms,peak};
}
async function run(id,text,stream,save){
 const before=await stats();const t=perf.now();
 const res=await fetch(base+(stream?'/tts/stream':'/tts'),payload({text}));assert.equal(res.status,200);
 let first=0;const chunks=[];for await(const b of res.body){if(!first)first=perf.now()-t;chunks.push(b);}
 const wall=perf.now()-t;const data=Buffer.concat(chunks);const after=await stats();
 if(!stream){assert.equal(data.toString('ascii',0,4),'RIFF');assert.equal(data.readUInt32LE(4)+8,data.length);assert.equal(data.readUInt16LE(20),1);assert.equal(data.readUInt16LE(22),1);assert.equal(data.readUInt32LE(24),24000);assert.equal(data.readUInt16LE(34),16);assert.equal(data.readUInt32LE(40)+44,data.length);if(save)await fs.writeFile('examples/audio/rust-'+id+'.wav',data);}
 const measures=analyze(stream?data:data.subarray(44));
 const cpu=Number(process.env.CLK_TCK);
 const row={id,stream,firstMs:first,wallMs:wall,...measures,rtf:wall/(1000*measures.seconds),chunks:chunks.length,rssPeakKiB:after.rssPeakKiB,cpuPct:cpu?(after.ticks-before.ticks)*100000/(cpu*wall):undefined};
 results.runs.push(row);return row;
}
assert.equal((await fetch(base+'/ready',{headers:auth})).status,200);
results.startMetrics=await (await fetch(base+'/metrics',{headers:auth})).text();
for(let i=0;i<3;i++){for(const [id,text] of corpus.slice(0,2)){await run(id,text,true,false);await persist();}}
if(process.env.SWEEP_ONLY!=='1'){
 for(const [id,text] of corpus)await run(id,text,false,true);
 for(const data of [{text:''},{text:'   '},{text:'… 🎵'},{text:'а'.repeat(2001)},{text:'тест',speed:0},{text:'Hello',lang:'en'},{text:'тест',format:'bad'},{text:'тест',voice:'other'}]){
 const res=await fetch(base+'/tts',payload(data));assert.equal(res.status,422);await res.arrayBuffer();}
 for (const body of ['', Buffer.from([0xff, 0x0a]), 'а'.repeat(2001)]) {
   const response = await fetch(base+'/tts/stream', {
     method:'POST', headers:{...auth,'Content-Type':'text/plain'}, body,
     signal:AbortSignal.timeout(5000),
   });
   assert.equal(response.status,422);
   assert.equal((await response.json()).error,'invalid_input');
 }
 const mp=await fetch(base+'/tts',payload({text:'Проверка.',speed:1.25,format:'mp3'}));assert.equal(mp.status,200);assert.equal(mp.headers.get('content-type'),'audio/mpeg');await fs.writeFile('examples/audio/rust-fast.mp3',Buffer.from(await mp.arrayBuffer()));
 const queueStart=await(await fetch(base+'/metrics',{headers:auth})).text();
 await Promise.all([run('concurrent-a',corpus[0][1],false,false),run('concurrent-b',corpus[0][1],false,false)]);
 results.afterConcurrent=await(await fetch(base+'/metrics',{headers:auth})).text();results.beforeConcurrent=queueStart;
 const ac=new AbortController();const req=await fetch(base+'/tts/stream',{...payload({text:corpus[1][1].repeat(3)},{'X-Request-Id':'probe-cancel'}),signal:ac.signal});
 const reader=req.body.getReader();assert.ok((await reader.read()).value.length);const cancel=await fetch(base+'/tts/probe-cancel',{method:'DELETE',headers:auth});assert.equal(cancel.status,202);ac.abort();await reader.cancel().catch(()=>{});
 // Keep upload open until audio is received. This also tests a client disconnect mid-input.
 await new Promise((resolve,reject)=>{const req=http.request(base+'/tts/stream',{method:'POST',headers:{...auth,'Content-Type':'text/plain'}},res=>{try{assert.equal(res.statusCode,200)}catch(e){reject(e);return}res.once('data',chunk=>{try{assert.ok(chunk.length);results.duplexFirstChunk=chunk.length;req.end('Следующая фраза.');}catch(e){reject(e)}});res.on('error',reject);res.on('end',resolve);});req.on('error',reject);req.setTimeout(25000,()=>req.destroy(Error('duplex timeout')));req.write('Первая фраза.\n');});
 results.recovery=await run('after-cancel','Связь восстановлена.',true,false);
 results.functional='passed';
}
results.endMetrics=await(await fetch(base+'/metrics',{headers:auth})).text();await persist();
console.log(file,results.runs.length,results.functional??'sweep');
