import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cancelAudioFileTrial, copyExplicitText, getCredentials, transcribeAudioFileTrial, type AudioFileTranscript } from '../lib/ipc';
import type { CredentialsStatus } from '../lib/types';
import sampleUrl from '../../docs/assets/voice-example/voice-sample.wav?url';
import { detectOS } from './WindowChrome';
import { Btn } from '../pages/_atoms';
import './StyleTextTrial.css';

const MAX_WAV = 60 * 32_000 + 65_536;
export function AudioFileTrial({ onClose, onOpenProviders }: { onClose: () => void; onOpenProviders?: () => void }) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const generation = useRef(0);
  const requestId = useRef<string | null>(null);
  const copyBusy = useRef(false);
  const selecting = useRef(false);
  const [selection, setSelection] = useState<{ name: string; bytes: number[]; sample: boolean } | null>(null);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<CredentialsStatus | null>(null);
  const [statusFailed, setStatusFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<(AudioFileTranscript & { source: string }) | null>(null);
  const [text, setText] = useState('');
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const [closeConfirm, setCloseConfirm] = useState(false);
  const supported = status?.activeAsrProvider === 'volcengine';
  const ready = supported && status.asrConfigured;
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
    if (copyBusy.current || selecting.current) return;
    if (requestId.current) { void cancelAudioFileTrial(requestId.current).catch(() => {}); }
    if (result && text.trim() && !copied) setCloseConfirm(true);
    else onClose();
  };
  const select = async (name: string, read: () => Promise<ArrayBuffer>, size?: number, sample = false) => {
    if (selecting.current || requestId.current || copyBusy.current || result) return;
    selecting.current = true; setLoading(true); setError('');
    const version = ++generation.current;
    try {
      if (size !== undefined && size > MAX_WAV) throw new Error('audio_file_too_long');
      const bytes = new Uint8Array(await read());
      if (!bytes.length || bytes.length > MAX_WAV) throw new Error('audio_file_too_long');
      if (generation.current === version) setSelection({ name, bytes: Array.from(bytes), sample });
    } catch (cause) {
      if (generation.current === version) { setSelection(null); setError(String(cause).includes('audio_file_too_long') ? t('audioFile.tooLong') : t('audioFile.loadFailed')); }
    } finally {
      selecting.current = false;
      if (generation.current === version) setLoading(false);
    }
  };
  const transcribe = async () => {
    if (!selection || !ready || requestId.current || copyBusy.current || selecting.current || result) return;
    const id = crypto.randomUUID(); requestId.current = id;
    const version = ++generation.current;
    const source = selection.name;
    setPending(true); setError('');
    try {
      const value = await transcribeAudioFileTrial(id, selection.bytes);
      if (version === generation.current) { setResult({ ...value, source }); setText(value.text); setCopied(false); }
    } catch (cause) {
      if (version === generation.current) {
        const code = String(cause);
        const key = ['format', 'too_long', 'cancelled', 'busy', 'provider', 'recording', 'setup', 'proxy', 'empty', 'timeout', 'native'].find(part => code === `audio_file_${part}` || code === `Error: audio_file_${part}`);
        setError(t(key === 'too_long' ? 'audioFile.tooLong' : key ? `audioFile.${key}` : 'audioFile.failed'));
      }
    } finally {
      if (requestId.current === id) requestId.current = null;
      if (version === generation.current) setPending(false);
    }
  };
  const copy = async () => {
    if (!text.trim() || copyBusy.current || requestId.current) return;
    copyBusy.current = true; setCopying(true); setCopied(false); setError('');
    const version = generation.current;
    try { await copyExplicitText(text, detectOS() === 'win'); if (version === generation.current) { setCopied(true); setCloseConfirm(false); } }
    catch { if (version === generation.current) setError(t('audioFile.copyFailed')); }
    finally { copyBusy.current = false; if (version === generation.current) setCopying(false); }
  };
  return <dialog ref={dialog} className="ol-style-trial" aria-labelledby="audio-file-title" onCancel={event => { event.preventDefault(); close(); }}>
    <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'center' }}>
      <h2 id="audio-file-title" style={{ margin: 0, fontSize: 20 }}>{t('audioFile.title')}</h2>
      <Btn variant="ghost" disabled={copying || loading} onClick={close}>{t('audioFile.close')}</Btn>
    </div>
    <p className="ol-style-trial-hint">{t('audioFile.intro')}</p>
    <p role="note" className="ol-style-trial-hint">{t('audioFile.cost')}</p>
    <div role="status" className="ol-style-trial-hint">
      {statusFailed ? t('audioFile.statusFailed') : !status ? t('audioFile.checking') : !supported ? t('audioFile.provider') : !ready ? t('audioFile.setup') : t('audioFile.ready')}
      {(statusFailed || (status && !ready)) && <Btn size="sm" variant="ghost" onClick={() => setRetry(x => x + 1)}>{t('audioFile.retry')}</Btn>}
      {status && !ready && onOpenProviders && <Btn size="sm" variant="ghost" onClick={onOpenProviders}>{t('audioFile.settings')}</Btn>}
    </div>
    <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap', alignItems: 'center', margin: '16px 0' }}>
      <label>{t('audioFile.choose')} <input aria-label={t('audioFile.choose')} type="file" accept=".wav,audio/wav" disabled={loading || pending || !!result || copying} onChange={event => {
        const file = event.target.files?.[0];
        if (file) void select(file.name, () => file.arrayBuffer(), file.size);
        event.target.value = '';
      }} /></label>
      <Btn size="sm" variant="ghost" disabled={loading || pending || !!result || copying} onClick={() => void select(t('audioFile.sampleName'), async () => {
        const response = await fetch(sampleUrl); if (!response.ok) throw Error('sample'); return response.arrayBuffer();
      }, undefined, true)}>{t('audioFile.sample')}</Btn>
    </div>
    <p className="ol-style-trial-hint">{loading ? t('audioFile.loading') : selection?.name ?? t('audioFile.noFile')}</p>
    {selection?.sample && <details className="ol-style-trial-details"><summary>{t('audioFile.sampleWords')}</summary><p>Hi Sam, let's meet on Thursday at three. I'll send the notes before lunch. Please keep the budget at fifty dollars, and tell me if the time needs to change.</p><audio controls preload="none" src={sampleUrl} aria-label={t('audioFile.sampleName')} /></details>}
    {!result && <Btn variant="blue" disabled={!selection || !ready || loading || pending} onClick={() => void transcribe()}>{t(pending ? 'audioFile.processing' : 'audioFile.submit')}</Btn>}
    {pending && <Btn variant="ghost" onClick={() => { if (requestId.current) void cancelAudioFileTrial(requestId.current).catch(() => {}); }}>{t('audioFile.stop')}</Btn>}
    {result && <section aria-label={t('audioFile.result')} style={{ marginTop: 16 }}>
      <h3 style={{ fontSize: 15 }}>{t('audioFile.result')}</h3>
      <p className="ol-style-trial-hint">{t('audioFile.receipt', { source: result.source, duration: (result.audioDurationMs / 1000).toFixed(1), time: (result.elapsedMs / 1000).toFixed(1) })}</p>
      <textarea aria-label={t('audioFile.result')} className="ol-style-trial-result-body ol-style-trial-result-editor" value={text} disabled={copying} onChange={event => { setText(event.target.value); setCopied(false); }} />
      <p className="ol-style-trial-hint">{t('audioFile.retention')}</p>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 10, alignItems: 'center' }}>
        <Btn variant="blue" disabled={copying || !text.trim()} onClick={() => void copy()}>{t(copying ? 'audioFile.copying' : copied ? 'audioFile.copied' : 'audioFile.copy')}</Btn>
        <Btn variant="ghost" disabled={copying} onClick={() => { setCloseConfirm(false); setResult(null); setText(''); setCopied(false); setError(''); }}>{t('audioFile.discard')}</Btn>
      </div>
    </section>}
    {error && <p role="alert" className="ol-style-trial-hint" style={{ color: 'var(--ol-red)' }}>{error}</p>}
    {closeConfirm && <div role="alert" style={{ marginTop: 14 }}><p>{t('audioFile.confirmClose')}</p><Btn onClick={() => setCloseConfirm(false)}>{t('audioFile.keep')}</Btn> <Btn variant="ghost" onClick={onClose}>{t('audioFile.closeWithoutCopy')}</Btn></div>}
  </dialog>;
}
