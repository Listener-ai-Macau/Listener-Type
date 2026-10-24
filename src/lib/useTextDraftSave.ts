import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { saveTextDraft, type SavedTextDraft } from './ipc';

/** Shared explicit-save ownership, including retries after an uncertain reply. */
export function useTextDraftSave({ name, original, text, disabled, onBusy, onSaved, onSavedRecord, onUnsaved }: {
  name: string; original: string; text: string; disabled: boolean; onBusy: (busy: boolean) => void;
  onSaved?: (text: string) => void; onSavedRecord?: (record: SavedTextDraft) => void; onUnsaved?: () => void;
}) {
  const { t } = useTranslation();
  const signature = JSON.stringify([name, original, text]);
  const request = useRef<{ signature: string; id: string } | null>(null);
  const running = useRef(false), alive = useRef(true);
  const [saving, setSaving] = useState(false), [saved, setSaved] = useState(''), [error, setError] = useState('');
  const [savedId, setSavedId] = useState('');
  useEffect(() => { alive.current = true; return () => { alive.current = false; onBusy(false); }; }, []);
  useEffect(() => { setError(''); }, [signature]);
  const save = async () => {
    if (running.current || disabled || !text.trim() || saved === signature) return;
    if (request.current?.signature !== signature) request.current = { signature, id: crypto.randomUUID() };
    running.current = true; setSaving(true); setError(''); onBusy(true);
    try {
      const value = await saveTextDraft(request.current.id, Array.from(name).slice(0, 80).join(''), original, text);
      if (alive.current) { setSaved(signature); setSavedId(value.id); onSaved?.(text); onSavedRecord?.(value); }
    } catch (cause) {
      if (alive.current) setError(t(String(cause).includes('saved_draft_limit') ? 'textDraft.full' : 'textDraft.failed'));
    } finally {
      running.current = false;
      if (alive.current) { setSaving(false); onBusy(false); }
    }
  };
  const invalidate = (id: string) => {
    if (id !== savedId) return;
    setSaved(''); setSavedId(''); onUnsaved?.();
  };
  return { save, saving, isSaved: saved === signature, savedId, error, invalidate };
}
