/** Decode locally with the operating system's WebView codecs; never upload here. */
export async function compressedAudioToWav(buffer: ArrayBuffer): Promise<Uint8Array> {
  if (!buffer.byteLength) throw new Error('audio_file_empty_file');
  let decoded: AudioBuffer;
  try {
    const decoder = new OfflineAudioContext(1, 1, 16_000);
    decoded = await decoder.decodeAudioData(buffer);
  } catch {
    throw new Error('audio_file_compressed_format');
  }
  if (!Number.isFinite(decoded.duration) || decoded.duration <= 0) throw new Error('audio_file_compressed_format');
  if (decoded.duration > 60) throw new Error('audio_file_too_long');
  const length = Math.ceil(decoded.duration * 16_000);
  const renderer = new OfflineAudioContext(1, length, 16_000);
  const source = renderer.createBufferSource();
  source.buffer = decoded;
  source.connect(renderer.destination);
  source.start();
  const samples = (await renderer.startRendering()).getChannelData(0);
  const bytes = new Uint8Array(44 + samples.length * 2);
  const view = new DataView(bytes.buffer);
  const put = (offset: number, text: string) => { for (let i = 0; i < text.length; i++) bytes[offset + i] = text.charCodeAt(i); };
  put(0, 'RIFF'); view.setUint32(4, bytes.length - 8, true); put(8, 'WAVE'); put(12, 'fmt ');
  view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, 16_000, true); view.setUint32(28, 32_000, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  put(36, 'data'); view.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const value = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(44 + i * 2, value < 0 ? Math.round(value * 32768) : Math.round(value * 32767), true);
  }
  return bytes;
}
