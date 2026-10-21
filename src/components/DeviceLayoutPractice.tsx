import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { DeviceCustomKeyMapping, DeviceKeyProfile } from '../lib/types';
import { layoutKeyIds } from '../lib/deviceKeyLayouts';

/** A scoped software practice field. Never dispatches shortcuts, IPC or clipboard calls. */
export function DeviceLayoutPractice({ profile, onInteract }: { profile: DeviceKeyProfile; onInteract?: () => void }) {
  const { t } = useTranslation();
  const field = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState('');
  const [buffer, setBuffer] = useState<string | null>(null);
  const [status, setStatus] = useState('');
  const undo = useRef<Array<{ text: string; start: number; end: number }>>([]);
  const selection = useRef({ start: 0, end: 0 });
  const range = () => ({ start: field.current?.selectionStart ?? text.length, end: field.current?.selectionEnd ?? text.length });
  const place = (start: number, end = start) => {
    selection.current = { start, end };
    requestAnimationFrame(() => { field.current?.focus(); field.current?.setSelectionRange(start, end); });
  };
  const replace = (value: string) => {
    const { start, end } = range();
    const next = text.slice(0, start) + value + text.slice(end);
    if (next.length > 4000) { setStatus(t('devicePractice.tooLong')); return; }
    undo.current.push({ text, start, end });
    if (undo.current.length > 32) undo.current.shift();
    setText(next); place(start + value.length); setStatus(t('devicePractice.inserted'));
  };
  const copy = (cut = false) => {
    const { start, end } = range();
    if (start === end) { setStatus(t('devicePractice.selectFirst')); return; }
    setBuffer(text.slice(start, end));
    if (cut) replace('');
    setStatus(t('devicePractice.copied'));
  };
  const paste = () => buffer === null ? setStatus(t('devicePractice.copyFirst')) : replace(buffer);
  const goBack = () => {
    const previous = undo.current.pop();
    if (!previous) { setStatus(t('devicePractice.nothingToUndo')); return; }
    setText(previous.text); place(previous.start, previous.end); setStatus(t('devicePractice.undone'));
  };
  const press = (mapping: DeviceCustomKeyMapping) => {
    onInteract?.();
    if (mapping.action === 'pasteTemplate') return replace(mapping.pasteTemplate);
    if (mapping.action === 'copyShortcut') return copy();
    if (mapping.action === 'pasteShortcut') return paste();
    if (mapping.action === 'undoShortcut') return goBack();
    if (mapping.action === 'disabled') return setStatus(t('devicePractice.disabled'));
    if (mapping.action === 'switchStyle') return setStatus(t('keyActionHelp.switchStylePractice'));
    if (mapping.action === 'sendShortcut' && mapping.shortcut) {
      const primary = mapping.shortcut.primary.toLowerCase();
      const mods = mapping.shortcut.modifiers;
      const command = mods.length === 1 && ['ctrl','super','meta','cmd'].includes(mods[0].toLowerCase());
      if (command && primary === 'a') { place(0, text.length); setStatus(t('devicePractice.selected')); return; }
      if (command && primary === 'c') return copy();
      if (command && primary === 'x') return copy(true);
      if (command && primary === 'v') return paste();
      if (command && primary === 'z') return goBack();
      if (!mods.length && ['enter','tab','space'].includes(primary)) return replace({enter:'\n',tab:'\t',space:' '}[primary]!);
    }
    setStatus(t('devicePractice.notSimulated'));
  };
  return <section className="ol-device-layout-practice" aria-label={t('devicePractice.title')}>
    <h3>{t('devicePractice.title')}</h3>
    <p className="ol-device-layout-hint">{t('devicePractice.description')}</p>
    <label>{t('devicePractice.text')}<textarea ref={field} rows={5} maxLength={4000} aria-label={t('devicePractice.text')} value={text}
      placeholder={t('devicePractice.placeholder')}
      onSelect={() => { selection.current = range(); }}
      onChange={event => {
        onInteract?.();
        undo.current.push({ text, ...selection.current });
        if (undo.current.length > 32) undo.current.shift();
        setText(event.target.value); selection.current = { start: event.target.selectionStart, end: event.target.selectionEnd }; setStatus('');
      }} /></label>
    <div className="ol-device-layout-pad ol-device-practice-keys">
      {layoutKeyIds.filter(id => id !== 'knob').map(id => <button key={id} type="button" onMouseDown={event => event.preventDefault()} onClick={() => press(profile.deviceCustomKeys[id])}>
        <strong>{t('deviceLayouts.key', {number:id.slice(3)})}</strong><span>{t(`settings.deviceKeys.actions.${profile.deviceCustomKeys[id].action}`)}</span>
        {profile.deviceCustomKeys[id].action === 'pasteTemplate' && <span className="ol-device-key-phrase" title={profile.deviceCustomKeys[id].pasteTemplate}>{profile.deviceCustomKeys[id].pasteTemplate}</span>}
      </button>)}
    </div>
    <p role="status" className="ol-device-layout-practice-status">{status || t('devicePractice.ready')}</p>
    <p className="ol-device-layout-hint">{t('devicePractice.scope')}</p>
  </section>;
}
