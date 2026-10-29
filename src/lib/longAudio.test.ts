import assert from 'node:assert/strict';
import { chunkWav, planAudioChunks, wavPcmRange } from './longAudio.ts';

// Two minutes of identifiable samples, including a nonzero final sample.
const samples = 125 * 16_000 + 73;
const wav = new Uint8Array(44 + samples * 2);
const v = new DataView(wav.buffer);
const put = (p: number, s: string) => { for (let i=0; i<s.length; i++) wav[p+i] = s.charCodeAt(i); };
put(0,'RIFF');v.setUint32(4,wav.length-8,true);put(8,'WAVE');put(12,'fmt ');
v.setUint32(16,16,true);v.setUint16(20,1,true);v.setUint16(22,1,true);v.setUint32(24,16000,true);
v.setUint32(28,32000,true);v.setUint16(32,2,true);v.setUint16(34,16,true);put(36,'data');v.setUint32(40,samples*2,true);
for(let i=0;i<samples;i++)v.setInt16(44+i*2,(i%32767)-16383,true);
const range=wavPcmRange(wav), plan=planAudioChunks(samples);
assert.equal(plan[0].start,0);assert.equal(plan.at(-1)?.end,samples);
for(let i=0;i<plan.length;i++) {
  const part=plan[i], encoded=chunkWav(wav,range,part), decoded=wavPcmRange(encoded);
  assert.ok(part.end-part.start<=55*16000);
  assert.deepEqual(encoded.subarray(decoded.start,decoded.end),wav.subarray(range.start+part.start*2,range.start+part.end*2));
  if(i)assert.equal(plan[i-1].end-part.start,16000,'one-second overlap prevents gaps');
}
assert.equal(new DataView(chunkWav(wav,range,plan.at(-1)!).buffer).getInt16(44+(plan.at(-1)!.end-plan.at(-1)!.start-1)*2,true),v.getInt16(wav.length-2,true));
for(const invalid of [new Uint8Array(),wav.subarray(0,wav.length-1)])assert.throws(()=>wavPcmRange(invalid));
const stereo=wav.slice();new DataView(stereo.buffer).setUint16(22,2,true);assert.throws(()=>wavPcmRange(stereo));
assert.throws(()=>planAudioChunks(0));
console.log('PASS PCM chunks preserve every sample and final tail, overlap exactly one second, enforce WAV format and reject truncation');
