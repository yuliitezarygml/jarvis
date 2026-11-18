// Local evaluation only. JARVIS_WESPEAKER points to wespeaker_en_voxceleb_resnet34.onnx.
const fs=require('node:fs');
const {execFileSync}=require('node:child_process');
const sherpa=require('sherpa-onnx-node');
const model=process.env.JARVIS_WESPEAKER;
const extractor=new sherpa.SpeakerEmbeddingExtractor({model,numThreads:2,provider:'cpu',debug:0});
function vec(file){
 const buf=execFileSync('ffmpeg',['-nostdin','-v','error','-i',file,'-ac','1','-ar','16000','-f','f32le','pipe:1'],{maxBuffer:16000000});
 const samples=new Float32Array(buf.buffer.slice(buf.byteOffset,buf.byteOffset+buf.length));
 const stream=extractor.createStream();stream.acceptWaveform({sampleRate:16000,samples});stream.inputFinished();
 if(!extractor.isReady(stream))throw Error(file+' too short');
 return extractor.compute(stream,false);
}
function cos(a,b){let xy=0,xx=0,yy=0;for(let i=0;i<a.length;i++){xy+=a[i]*b[i];xx+=a[i]*a[i];yy+=b[i]*b[i];}return xy/Math.sqrt(xx*yy);}
const ref=vec('.assets/jarvis-ru.wav');const rows=[];
for(const [id] of JSON.parse(fs.readFileSync('audit/corpus.json'))){const a=vec('examples/audio/old-'+id+'.wav'),b=vec('examples/audio/rust-'+id+'.wav');rows.push({id,oldVsReference:cos(a,ref),rustVsReference:cos(b,ref),oldVsRust:cos(a,b)});}
fs.writeFileSync('audit/voice-similarity.json',JSON.stringify({model:'wespeaker_en_voxceleb_resnet34.onnx',sha256:require('node:crypto').createHash('sha256').update(fs.readFileSync(model)).digest('hex'),rows},null,2));
