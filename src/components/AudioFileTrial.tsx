import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cancelAudioFileTrial, copyExplicitText, getCredentials, transcribeAudioFileTrial, type AudioFileTranscript } from '../lib/ipc';
import type { CredentialsStatus } from '../lib/types';
import sampleUrl from '../../docs/assets/voice-example/voice-sample.wav?url';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';
import { SaveTextDraft } from './SaveTextDraft';
import { QuickStyleTextTrial } from './QuickStyleTextTrial';
import { ProviderSettingsDialog } from './ProviderSettingsDialog';
import './StyleTextTrial.css';
import { compressedAudioToWav } from '../lib/compressedAudio';
import { audioChunkLabel, chunkWav, planAudioChunks, wavPcmRange, type AudioChunk } from '../lib/longAudio';

const MAX_WAV = 60 * 32_000 + 65_536;
function validateSelectedFileLength(length: number): void {
  if (length === 0) throw new Error('audio_file_empty_file');
}
type AudioSelection = { name: string; bytes: Uint8Array; sample: boolean; range?: { start: number; end: number }; chunks?: AudioChunk[] };
type CompletedChunk = { label: string; text: string; elapsedMs: number; unrecognized?: boolean };
const partialText = (chunks: CompletedChunk[]) => chunks.map(x => `[${x.label}]\n${x.text}`).join('\n\n');
export function AudioFileTrial({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const scrollArea = useRef<HTMLDivElement>(null);
  const resultArea = useRef<HTMLElement>(null);
  const generation = useRef(0);
  const requestId = useRef<string | null>(null);
  const copyBusy = useRef(false);
  const saveBusy = useRef(false);
  const selecting = useRef(false);
  const running = useRef(false), stopRequested = useRef(false), completed = useRef<CompletedChunk[]>([]);
  const [selection, setSelection] = useState<AudioSelection | null>(null);
  const [partial, setPartial] = useState<CompletedChunk[]>([]);
  const [partialCopied, setPartialCopied] = useState(false);
  const [discardPartial, setDiscardPartial] = useState(false);
  const [emptyChunk, setEmptyChunk] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<CredentialsStatus | null>(null);
  const [statusFailed, setStatusFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<(AudioFileTranscript & { source: string; unrecognizedSegments?: number }) | null>(null);
  const [text, setText] = useState('');
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedText, setSavedText] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [closeConfirm, setCloseConfirm] = useState(false);
  const [textWorkspaceOpen, setTextWorkspaceOpen] = useState(false);
  const [textWorkspaceDraft, setTextWorkspaceDraft] = useState('');
  const [speechSettingsOpen, setSpeechSettingsOpen] = useState(false);
  const supported = status?.activeAsrProvider === 'volcengine';
  const ready = supported && status.asrConfigured;
  useLayoutEffect(() => {
    const scroll = scrollArea.current, pane = resultArea.current;
    if (result && scroll && pane) scroll.scrollTop += pane.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
  }, [result]);
  useEffect(() => {
    let current = true;
    setStatus(null); setStatusFailed(false);
    void getCredentials().then(value => { if (current) setStatus(value); }).catch(() => { if (current) setStatusFailed(true); });
    return () => { current = false; };
  }, [retry]);
  useEffect(() => {
    dialog.current?.showModal();
    return () => {
      generation.current++;
      if (requestId.current) void cancelAudioFileTrial(requestId.current).catch(() => {});
      dialog.current?.close();
    };
  }, []);
  const close = () => {
    if (copyBusy.current || saveBusy.current || selecting.current) return;
    stopRequested.current = true;
    if (requestId.current) { void cancelAudioFileTrial(requestId.current).catch(() => {}); }
    if ((result && text.trim() && !copied && savedText !== text) || completed.current.length > 0) setCloseConfirm(true);
    else onClose();
  };
  const select = async (name: string, read: () => Promise<ArrayBuffer>, size?: number, sample = false, compressed = false) => {
    if (selecting.current || running.current || requestId.current || copyBusy.current || saveBusy.current || result || completed.current.length) return;
    selecting.current = true; setLoading(true); setError(''); setEmptyChunk(null); setDiscardPartial(false);
    const version = ++generation.current;
    try {
      if (size !== undefined) validateSelectedFileLength(size);
      const raw = await read();
      let bytes = compressed ? await compressedAudioToWav(raw) : new Uint8Array(raw);
      validateSelectedFileLength(bytes.length);
      let range: { start: number; end: number } | undefined;
      try { range = wavPcmRange(bytes); }
      catch (cause) { if (bytes.length > MAX_WAV || compressed) throw cause; }
      const samples = range ? (range.end - range.start) / 2 : 0;
      const chunks = samples > 60 * 16_000 ? planAudioChunks(samples) : undefined;
      // A valid short WAV can contain a large metadata chunk. Preserve every
      // PCM sample while removing only container metadata that would exceed
      // the backend's per-request byte limit.
      if (range && !chunks && bytes.length > MAX_WAV) {
        bytes = chunkWav(bytes, range, { start: 0, end: samples });
        range = wavPcmRange(bytes);
      }
      if (generation.current === version) setSelection({ name, bytes, sample, range, chunks });
    } catch (cause) {
      if (generation.current === version) {
        const code = cause instanceof Error ? cause.message : String(cause);
        setSelection(null);
        setError(t(code === 'audio_file_empty_file' ? 'audioFile.emptyFile' : code === 'audio_file_format' ? 'audioFile.format' : code === 'audio_file_container_limit' ? 'audioFile.containerLimit' : code === 'audio_file_compressed_format' ? 'audioFile.compressedFormat' : 'audioFile.loadFailed'));
      }
    } finally {
      selecting.current = false;
      if (generation.current === version) setLoading(false);
    }
  };
  const transcribe = async () => {
    if (!selection || !ready || running.current || requestId.current || copyBusy.current || saveBusy.current || selecting.current || result) return;
    running.current = true; stopRequested.current = false;
    const version = ++generation.current;
    const source = selection.name;
    setPending(true); setError(''); setEmptyChunk(null);
    try {
      if (!selection.chunks || !selection.range) {
        const id = crypto.randomUUID(); requestId.current = id;
        const value = await transcribeAudioFileTrial(id, Array.from(selection.bytes));
        if (version === generation.current) { setResult({ ...value, source }); setText(value.text); setCopied(false); setSavedText(null); }
      } else {
        for (let index = completed.current.length; index < selection.chunks.length; index++) {
          if (stopRequested.current || version !== generation.current) throw new Error('audio_file_cancelled');
          const chunk = selection.chunks[index], wav = chunkWav(selection.bytes, selection.range, chunk);
          const id = crypto.randomUUID(); requestId.current = id;
          const value = await transcribeAudioFileTrial(id, Array.from(wav));
          if (version !== generation.current) return;
          completed.current = [...completed.current, { label: audioChunkLabel(chunk), text: value.text, elapsedMs: value.elapsedMs }];
          setPartial(completed.current); setPartialCopied(false);
          requestId.current = null;
          if (stopRequested.current && index + 1 < selection.chunks.length) throw new Error('audio_file_cancelled');
        }
        const output = partialText(completed.current), elapsedMs = completed.current.reduce((sum, item) => sum + item.elapsedMs, 0);
        const audioDurationMs = (selection.range.end - selection.range.start) / 32;
        const unrecognizedSegments = completed.current.filter(item => item.unrecognized).length;
        if (version === generation.current) { setResult({ text: output, audioDurationMs, elapsedMs, source, unrecognizedSegments }); setText(output); setCopied(false); setSavedText(null); completed.current = []; setPartial([]); }
      }
    } catch (cause) {
      if (version === generation.current) {
        const code = String(cause);
        const key = ['format', 'too_long', 'cancelled', 'busy', 'provider', 'recording', 'setup', 'proxy', 'empty', 'timeout', 'native'].find(part => code === `audio_file_${part}` || code === `Error: audio_file_${part}`);
        if (key === 'empty' && selection.chunks && !stopRequested.current) setEmptyChunk(completed.current.length);
        setError(t(key === 'too_long' ? 'audioFile.tooLong' : key ? `audioFile.${key}` : 'audioFile.failed'));
      }
    } finally {
      requestId.current = null; running.current = false;
      if (version === generation.current) setPending(false);
    }
  };
  const copy = async () => {
    if (!text.trim() || copyBusy.current || saveBusy.current || requestId.current) return;
    copyBusy.current = true; setCopying(true); setCopied(false); setError('');
    const version = generation.current;
    try { await copyExplicitText(text, detectOS() === 'win'); if (version === generation.current) { setCopied(true); setCloseConfirm(false); } }
    catch { if (version === generation.current) setError(t('audioFile.copyFailed')); }
    finally { copyBusy.current = false; if (version === generation.current) setCopying(false); }
  };
  const copyPartial = async () => {
    const value = partialText(completed.current);
    if (!value.trim() || copyBusy.current || saveBusy.current || running.current) return;
    copyBusy.current = true; setCopying(true); setError('');
    try { await copyExplicitText(value, detectOS() === 'win'); setPartialCopied(true); setCloseConfirm(false); }
    catch { setError(t('audioFile.copyFailed')); }
    finally { copyBusy.current = false; setCopying(false); }
  };
  return <><dialog ref={dialog} className="ol-style-trial" aria-labelledby="audio-file-title" onCancel={event => {
    // A file chooser also emits a bubbling `cancel` when no file is selected.
    // Only the dialog's own cancel action should discard this workspace.
    if (event.target !== event.currentTarget) return;
    event.preventDefault(); close();
  }}>
    <div className="ol-style-trial-header">
      <h2 id="audio-file-title" style={{ margin: 0, fontSize: 20 }}>{t('audioFile.title')}</h2>
      <Btn variant="ghost" disabled={copying || saving || loading} onClick={close}>{t('audioFile.close')}</Btn>
    </div>
    <div ref={scrollArea} className="ol-style-trial-scroll" data-audio-file-scroll>
    <p className="ol-style-trial-hint">{t('audioFile.intro')}</p>
    <p role="note" className="ol-style-trial-hint">{t('audioFile.cost')}</p>
    <div role="status" className="ol-style-trial-hint">
      {statusFailed ? t('audioFile.statusFailed') : !status ? t('audioFile.checking') : !supported ? t('audioFile.provider') : !ready ? t('audioFile.setup') : t('audioFile.ready')}
      {(statusFailed || (status && !ready)) && <Btn size="sm" variant="ghost" onClick={() => setRetry(x => x + 1)}>{t('audioFile.retry')}</Btn>}
      {status && !ready && <Btn size="sm" variant="ghost" onClick={() => setSpeechSettingsOpen(true)}>{t('audioFile.settings')}</Btn>}
    </div>
    <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap', alignItems: 'center', margin: '16px 0' }}>
      <label>{t('audioFile.choose')} <input aria-label={t('audioFile.choose')} type="file" accept=".wav,audio/wav" disabled={loading || pending || !!result || copying || partial.length > 0} onChange={event => {
        const file = event.target.files?.[0];
        if (file) void select(file.name, () => file.arrayBuffer(), file.size);
        event.target.value = '';
      }} /></label>
      <label>{t('audioFile.chooseCompressed')} <input aria-label={t('audioFile.chooseCompressed')} type="file" accept=".mp3,.m4a,audio/mpeg,audio/mp4" disabled={loading || pending || !!result || copying || partial.length > 0} onChange={event => {
        const file = event.target.files?.[0];
        if (file) void select(file.name, () => file.arrayBuffer(), file.size, false, true);
        event.target.value = '';
      }} /></label>
      <Btn size="sm" variant="ghost" disabled={loading || pending || !!result || copying || partial.length > 0} onClick={() => void select(t('audioFile.sampleName'), async () => {
        const response = await fetch(sampleUrl); if (!response.ok) throw Error('sample'); return response.arrayBuffer();
      }, undefined, true)}>{t('audioFile.sample')}</Btn>
    </div>
    <p className="ol-style-trial-hint">{t('audioFile.compressedHint')}</p>
    <p className="ol-style-trial-hint">{loading ? t('audioFile.loading') : selection?.name ?? t('audioFile.noFile')}</p>
    {selection?.chunks && <p role="note" className="ol-style-trial-hint" data-long-audio-plan>{t('audioFile.longPlan', { count: selection.chunks.length })}</p>}
    {selection?.sample && <details className="ol-style-trial-details"><summary>{t('audioFile.sampleWords')}</summary><p>Hi Sam, let's meet on Thursday at three. I'll send the notes before lunch. Please keep the budget at fifty dollars, and tell me if the time needs to change.</p><audio controls preload="none" src={sampleUrl} aria-label={t('audioFile.sampleName')} /></details>}
    {partial.length > 0 && !result && <section aria-label={t('audioFile.partialTitle')} data-audio-file-partial>
      <h3>{t('audioFile.partialTitle')}</h3>
      <p role="status" className="ol-style-trial-hint">{t('audioFile.partialProgress', { done: partial.length, total: selection?.chunks?.length ?? 0 })}</p>
      <textarea readOnly aria-label={t('audioFile.partialTitle')} className="ol-style-trial-result-body ol-style-trial-result-editor" rows={8} value={partialText(partial)} />
      {Array.from(partialText(partial)).length > 100_000 && <p className="ol-style-trial-hint">{t('audioFile.tooLongToSave')}</p>}
      <div className="ol-style-trial-primary-actions">
        <Btn disabled={pending || copying || saving} onClick={() => void copyPartial()}>{t(partialCopied ? 'audioFile.partialCopied' : 'audioFile.copyPartial')}</Btn>
        <SaveTextDraft compact name={selection?.name ?? ''} original={partialText(partial)} text={partialText(partial)} disabled={pending || copying || Array.from(partialText(partial)).length > 100_000}
          onBusy={value => { saveBusy.current = value; setSaving(value); }}
          onSaved={() => setCloseConfirm(false)} />
        <Btn variant="ghost" disabled={pending || copying || saving} onClick={() => setDiscardPartial(true)}>{t('audioFile.discardPartial')}</Btn>
      </div>
      {discardPartial && <div role="alert"><p>{t('audioFile.discardPartialConfirm')}</p>
        <Btn onClick={() => setDiscardPartial(false)}>{t('audioFile.keep')}</Btn>{' '}
        <Btn variant="ghost" onClick={() => { completed.current = []; setPartial([]); setPartialCopied(false); setDiscardPartial(false); setEmptyChunk(null); setSelection(null); setError(''); }}>{t('audioFile.discardPartialConfirmButton')}</Btn>
      </div>}
    </section>}
    {emptyChunk !== null && selection?.chunks?.[emptyChunk] && !pending && <div role="note" data-audio-empty-segment>
      <p className="ol-style-trial-hint">{t('audioFile.emptySegmentHint', { range: audioChunkLabel(selection.chunks[emptyChunk]) })}</p>
      <Btn disabled={copying || saving} onClick={() => {
        if (emptyChunk !== completed.current.length || !selection.chunks?.[emptyChunk]) return;
        completed.current = [...completed.current, { label: audioChunkLabel(selection.chunks[emptyChunk]), text: t('audioFile.emptySegmentMarker'), elapsedMs: 0, unrecognized: true }];
        setPartial(completed.current); setPartialCopied(false); setEmptyChunk(null); setError('');
      }}>{t('audioFile.skipEmptySegment')}</Btn>
    </div>}
    {result && <section ref={resultArea} aria-label={t('audioFile.result')} style={{ marginTop: 16 }}>
      <h3 style={{ fontSize: 15 }}>{t('audioFile.result')}</h3>
      <p className="ol-style-trial-hint">{t(selection?.chunks ? 'audioFile.longReceipt' : 'audioFile.receipt', { source: result.source, duration: (result.audioDurationMs / 1000).toFixed(1), time: (result.elapsedMs / 1000).toFixed(1) })}</p>
      {!!result.unrecognizedSegments && <p role="alert" className="ol-style-trial-hint">{t('audioFile.unrecognizedCount', { count: result.unrecognizedSegments })}</p>}
      <textarea aria-label={t('audioFile.result')} className="ol-style-trial-result-body ol-style-trial-result-editor" value={text} disabled={copying || saving} onChange={event => { setText(event.target.value); setCopied(false); setCloseConfirm(false); }} />
      {(Array.from(text).length > 100_000 || Array.from(result.text).length > 100_000) && <p className="ol-style-trial-hint">{t('audioFile.tooLongToSave')}</p>}
      <p className="ol-style-trial-hint">{t(text.length > 4000 ? 'audioFile.textWorkspaceLimit' : 'audioFile.textWorkspaceHint')}</p>
    </section>}
    </div>
    <div className="ol-style-trial-submit" data-audio-file-submit>
    {result && <>
      <p className="ol-style-trial-hint" data-audio-file-retention>{t(savedText === text ? 'textDraft.savedRetention' : 'audioFile.retention')}</p>
      <div className="ol-style-trial-primary-actions ol-style-trial-result-toolbar ol-audio-file-actions" data-audio-file-result-actions>
        <Btn variant="blue" disabled={copying || saving || !text.trim()} onClick={() => void copy()}>{t(copying ? 'audioFile.copying' : copied ? 'audioFile.copied' : 'audioFile.copy')}</Btn>
        <SaveTextDraft compact name={result.source} original={result.text} text={text} disabled={copying || Array.from(text).length > 100_000 || Array.from(result.text).length > 100_000}
          onBusy={value => {
            saveBusy.current = value; setSaving(value);
            if (value) { setError(''); setCloseConfirm(false); }
          }}
          onSaved={value => { setSavedText(value); setCloseConfirm(false); }} onUnsaved={() => setSavedText('')} />
        <Btn variant="ghost" disabled={copying || saving || !text.trim() || text.length > 4000} onClick={() => {
          if (copyBusy.current || saveBusy.current || !text.trim() || text.length > 4000) return;
          setTextWorkspaceDraft(text); setTextWorkspaceOpen(true);
        }}>{t('audioFile.openTextWorkspace')}</Btn>
        <Btn variant="ghost" disabled={copying || saving} onClick={() => {
          if (saveBusy.current || copyBusy.current) return;
          setCloseConfirm(false); setResult(null); setText(''); setCopied(false); setSavedText(null); setError('');
        }}>{t('audioFile.discard')}</Btn>
      </div>
    </>}
    {!result && <div className="ol-style-trial-submit-actions">
      <Btn variant="blue" disabled={!selection || !ready || loading || pending || copying || saving || discardPartial} onClick={() => void transcribe()}>{t(pending ? 'audioFile.processing' : partial.length ? 'audioFile.resume' : 'audioFile.submit')}</Btn>
      {pending && <Btn variant="ghost" onClick={() => { stopRequested.current = true; if (requestId.current) void cancelAudioFileTrial(requestId.current).catch(() => {}); }}>{t('audioFile.stop')}</Btn>}
    </div>}
    {error && <p role="alert" className="ol-style-trial-submit-error">{error}</p>}
    {closeConfirm && <div role="alert" style={{ marginTop: 14 }}><p>{t(partial.length ? 'audioFile.partialClose' : 'audioFile.confirmClose')}</p><Btn disabled={copying || saving} onClick={() => setCloseConfirm(false)}>{t('audioFile.keep')}</Btn> <Btn variant="ghost" disabled={copying || saving} onClick={() => {
      if (!saveBusy.current && !copyBusy.current) onClose();
    }}>{t('audioFile.closeWithoutCopy')}</Btn></div>}
    </div>
  </dialog>
    {speechSettingsOpen && <ProviderSettingsDialog kind="speech" onClose={() => {
      setSpeechSettingsOpen(false);
      setRetry(value => value + 1);
    }} />}
    {textWorkspaceOpen && <QuickStyleTextTrial draft={textWorkspaceDraft} onDraftChange={setTextWorkspaceDraft}
      onClose={() => setTextWorkspaceOpen(false)} />}
  </>;
}
