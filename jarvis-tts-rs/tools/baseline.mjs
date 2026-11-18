import fs from 'node:fs/promises';
import { performance } from 'node:perf_hooks';
const url=process.env.REFERENCE_URL ?? 'http://192.168.50.38:8080/api/tts';
const corpus=[
 ['short','Добрый вечер, сэр.'],['long','Диагностика завершена. Все системы работают в штатном режиме, связь с сервером устойчива. Готов принять следующую команду.'],
 ['english','Good morning, sir.'],['mixed','Джарвис, проверь Windows и CPU.'],['numbers','Температура 25 градусов, время 14:30.'],['punct','Сэр… Вы здесь? Отлично!']
];
await fs.writeFile('audit/corpus.json',JSON.stringify(corpus,null,2));
const rows=[];
for(const [id,text] of [...corpus,['empty',''],['invalid','... 🎵'],['oversize','я'.repeat(2001)],['lang','Hello'],['format','Тест'],['speed','Тест']]){
 const t=performance.now(); const res=await fetch(url,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({text,...(id==='lang'?{lang:'en'}:id==='format'?{format:'exe'}:id==='speed'?{speed:0}:{})}),signal:AbortSignal.timeout(180000)});
 const headersMs=performance.now()-t; const buf=Buffer.from(await res.arrayBuffer());
 const row={id,status:res.status,headersMs:Math.round(headersMs),wallMs:Math.round(performance.now()-t),cached:res.headers.get('x-voice-cache'),engineMs:res.headers.get('x-synthesis-ms')};
 if(res.ok){if(buf.toString('ascii',0,4)!=='RIFF'||buf.toString('ascii',36,40)!=='data'||buf.readUInt32LE(40)!==buf.length-44)throw Error('bad PCM WAV');row.hz=buf.readUInt32LE(24);row.seconds=buf.readUInt32LE(40)/buf.readUInt32LE(28);row.rtf=row.wallMs/(1000*row.seconds);await fs.writeFile('examples/audio/old-'+id+'.wav',buf);}
 rows.push(row);await fs.writeFile('audit/baseline.json',JSON.stringify(rows,null,2));console.log(row);
}
