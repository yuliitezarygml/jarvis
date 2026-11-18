import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
const fixtures=readFileSync('audit/legacy-text.jsonl','utf8').trim().split('\n').map(JSON.parse);
const output=spawnSync(process.env.JARVIS_BIN??'target/release/jarvis-tts-rs',['prepare'],{input:fixtures.map(r=>r[0]).join('\n')+'\n',encoding:'utf8',timeout:90000});
assert.equal(output.status,0,output.stderr);
assert.deepEqual(output.stdout.trimEnd().split('\n'),fixtures.map(r=>r[2]));
console.log(fixtures.length+' legacy normalized texts match, including stress and punctuation.');
