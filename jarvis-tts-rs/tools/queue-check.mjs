import assert from 'node:assert/strict';
import http from 'node:http';
import {setTimeout as delay} from 'node:timers/promises';
const base=process.env.TTS_URL??'http://127.0.0.1:8181';
const headers=process.env.JARVIS_BEARER_TOKEN?{Authorization:'Bearer '+process.env.JARVIS_BEARER_TOKEN}:{};
const clients=[];
try{
 for(let i=0;i<5;i++){
    const q=http.request(base+'/tts/stream',{method:'POST',headers:{...headers,'Content-Type':'text/plain','X-Request-Id':'hold-'+i}},r=>r.resume());
    q.on('error',()=>{});q.write('Жду');clients.push(q);
 }
 await delay(150);
 const r=await fetch(base+'/tts',{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:'{"text":"Тест"}'});
 assert.equal(r.status,429);
 for(let i=0;i<5;i++){assert.equal((await fetch(base+'/tts/hold-'+i,{method:'DELETE',headers})).status,202);}
}finally{for(const req of clients) req.destroy();}
await delay(100);
const r=await fetch(base+'/tts',{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:'{"text":"Свободно."}'});
assert.equal(r.status,200);assert.equal(Buffer.from(await r.arrayBuffer()).toString('ascii',0,4),'RIFF');
console.log('Five unfinished inputs admitted, sixth rejected, all cancelled, next call returned WAV.');
