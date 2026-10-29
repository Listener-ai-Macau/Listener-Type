/** Split already selected PCM audio without losing samples at a boundary.
 * Each request is shorter than the existing backend's 60-second contract.
 * Adjacent chunks overlap by one second so a word crossing a split remains
 * available; the UI labels the ranges instead of silently deduplicating. */
const RATE = 16_000;
const MAX_SAMPLES = 55 * RATE;
const OVERLAP = RATE;
export interface AudioChunk { start: number; end: number }

export function wavPcmRange(wav: Uint8Array): { start: number; end: number } {
  const bad = () => { throw new Error('audio_file_format'); };
  if (wav.length < 12) return bad();
  const v = new DataView(wav.buffer, wav.byteOffset, wav.byteLength);
  const id = (at: number) => String.fromCharCode(...wav.subarray(at, at + 4));
  if (id(0) !== 'RIFF' || id(8) !== 'WAVE') return bad();
  let fmt = false, data: { start: number; end: number } | undefined;
  for (let offset = 12; offset + 8 <= wav.length;) {
    const size = v.getUint32(offset + 4, true), start = offset + 8, end = start + size;
    if (end > wav.length) return bad();
    if (id(offset) === 'fmt ') {
      if (size < 16) return bad();
      fmt = v.getUint16(start, true) === 1 && v.getUint16(start + 2, true) === 1
        && v.getUint32(start + 4, true) === RATE && v.getUint16(start + 14, true) === 16;
    } else if (id(offset) === 'data') data = { start, end };
    offset = end + size % 2;
  }
  if (!fmt || !data || data.end === data.start || (data.end - data.start) % 2) return bad();
  return data;
}

export function planAudioChunks(samples: number): AudioChunk[] {
  if (!Number.isSafeInteger(samples) || samples <= 0) throw new Error('audio_file_format');
  const chunks: AudioChunk[] = [];
  for (let start = 0;;) {
    const end = Math.min(start + MAX_SAMPLES, samples);
    chunks.push({ start, end });
    if (end === samples) return chunks;
    start = end - OVERLAP;
  }
}

export function chunkWav(wav: Uint8Array, range: { start: number; end: number }, chunk: AudioChunk): Uint8Array {
  const start = range.start + chunk.start * 2, end = range.start + chunk.end * 2;
  if (start < range.start || end > range.end || start >= end || chunk.end - chunk.start > 60 * RATE) throw new Error('audio_file_format');
  const pcm = wav.subarray(start, end), output = new Uint8Array(44 + pcm.length), v = new DataView(output.buffer);
  const put = (at: number, value: string) => { for (let i = 0; i < value.length; i++) output[at + i] = value.charCodeAt(i); };
  put(0, 'RIFF'); v.setUint32(4, output.length - 8, true); put(8, 'WAVE'); put(12, 'fmt ');
  v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, RATE, true); v.setUint32(28, RATE * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  put(36, 'data'); v.setUint32(40, pcm.length, true); output.set(pcm, 44);
  return output;
}

export function audioChunkLabel(chunk: AudioChunk): string {
  const time = (sample: number) => {
    const ms = Math.floor(sample * 1000 / RATE), seconds = Math.floor(ms / 1000), fraction = ms % 1000;
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}${fraction ? '.' + String(fraction).padStart(3, '0').replace(/0+$/u, '') : ''}`;
  };
  return `${time(chunk.start)}–${time(chunk.end)}`;
}
