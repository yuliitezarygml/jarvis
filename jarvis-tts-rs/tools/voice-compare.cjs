// Offline evaluation of saved samples, independent of the synthesis server.
const fs = require('node:fs');
const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const sherpa = require('sherpa-onnx-node');

const model = process.env.JARVIS_WESPEAKER;
const rate = 16000;
const extractor = new sherpa.SpeakerEmbeddingExtractor({
  model, numThreads: 2, provider: 'cpu', debug: 0,
});

function samples(file) {
  const pcm = execFileSync('ffmpeg', [
    '-nostdin', '-v', 'error', '-i', file, '-ac', '1', '-ar', String(rate), '-f', 'f32le', 'pipe:1',
  ], { maxBuffer: 16000000 });
  return new Float32Array(pcm.buffer.slice(pcm.byteOffset, pcm.byteOffset + pcm.length));
}

function embedding(samples) {
  const stream = extractor.createStream();
  stream.acceptWaveform({ sampleRate: rate, samples });
  stream.inputFinished();
  if (!extractor.isReady(stream)) throw Error('Recording too short for speaker verification');
  return extractor.compute(stream, false);
}

function cosine(a, b) {
  let xy = 0, xx = 0, yy = 0;
  for (let i = 0; i < a.length; i++) {
    xy += a[i] * b[i];
    xx += a[i] * a[i];
    yy += b[i] * b[i];
  }
  return xy / Math.sqrt(xx * yy);
}

// Internal gaps with RMS below -40 dBFS for at least 120 ms. Ignore leading and trailing silence.
function pauses(pcm) {
  const step = rate / 100;
  const gaps = [];
  let started = false, silentFrames = 0;
  for (let i = 0; i + step <= pcm.length; i += step) {
    let energy = 0;
    for (let j = i; j < i + step; j++) energy += pcm[j] ** 2;
    if (energy / step < 0.0001) {
      if (started) silentFrames++;
    } else {
      if (silentFrames >= 12) gaps.push(silentFrames * 10);
      started = true;
      silentFrames = 0;
    }
  }
  return gaps;
}

const reference = embedding(samples('.assets/jarvis-ru.wav'));
const corpus = JSON.parse(fs.readFileSync('audit/corpus.json', 'utf8'));
const rows = corpus.map(([id]) => {
  const oldAudio = samples('examples/audio/old-' + id + '.wav');
  const rustAudio = samples('examples/audio/rust-' + id + '.wav');
  const old = embedding(oldAudio), rust = embedding(rustAudio);
  return {
    id, oldVsReference: cosine(old, reference), rustVsReference: cosine(rust, reference),
    oldVsRust: cosine(old, rust),
    oldSeconds: oldAudio.length / rate, rustSeconds: rustAudio.length / rate,
    oldInternalPausesMs: pauses(oldAudio), rustInternalPausesMs: pauses(rustAudio),
  };
});
fs.writeFileSync('audit/voice-similarity.json', JSON.stringify({
  model: 'wespeaker_en_voxceleb_resnet34.onnx',
  sha256: createHash('sha256').update(fs.readFileSync(model)).digest('hex'),
  rows,
}, null, 2) + '\n');
