import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deleteLocalDeviceLayout, listLocalDeviceLayouts, nativeDeviceLayoutsAvailable, saveLocalDeviceLayout } from '../lib/ipc';
import { defaultDeviceKeyProfile, layoutActions, layoutAppPages, layoutGestures, layoutKeyIds, writingDeviceKeyProfile } from '../lib/deviceKeyLayouts';
import type { DeviceCustomKeyId, DeviceCustomKeyMapping, DeviceKeyProfile, DeviceKnobRotationAction, DeviceLayout } from '../lib/types';
import { Btn } from '../pages/_atoms';
import './DeviceLayoutsDialog.css';
import { DeviceLayoutPractice } from './DeviceLayoutPractice';

type Group = typeof layoutGestures[number];
const primaryKeys = ['RightControl','LeftControl','RightShift','LeftShift','RightAlt','LeftAlt', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789', 'Enter','Tab','Space','Escape','Backspace','Delete','Insert','Home','End','PageUp','PageDown','ArrowUp','ArrowDown','ArrowLeft','ArrowRight', ...Array.from({length:12},(_,i)=>`F${i+1}`)];

export function DeviceLayoutsDialog({ initialProfile, initialKnob, initialPractice = false, onUseLayout, onClose }: {
  initialProfile?: DeviceKeyProfile;
  initialKnob?: DeviceKnobRotationAction;
  initialPractice?: boolean;
  onUseLayout?: (layout: DeviceLayout) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const alive = useRef(true);
  const [layouts, setLayouts] = useState<DeviceLayout[]>([]);
  const [editing, setEditing] = useState<DeviceLayout | null>(null);
  const [name, setName] = useState('');
  const [profile, setProfile] = useState(() => structuredClone(initialProfile ?? defaultDeviceKeyProfile()));
  const [knob, setKnob] = useState<DeviceKnobRotationAction>(initialKnob ?? 'systemVolume');
  const [key, setKey] = useState<DeviceCustomKeyId>('key1');
  const [group, setGroup] = useState<Group>('deviceCustomKeys');
  const [dirty, setDirty] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [busy, setBusy] = useState(false);
  const [practice, setPractice] = useState(initialPractice);
  const [practiceRevision, setPracticeRevision] = useState(0);
  const [message, setMessage] = useState('');
  const [confirmation, setConfirmation] = useState<{ text: string; action: () => void } | null>(null);
  const actionGroup = key === 'knob' ? 'deviceCustomKeys' : group;
  const mapping = profile[actionGroup][key];
  const errorText = (cause: unknown) => {
    const code = String(cause).replace(/^Error: /, '');
    const known = ['stale','invalid_name','limit','profile_too_large','invalid_file','file_too_large'];
    return known.some(value => code === `device_layout_${value}`) ? t(`deviceLayouts.errors.${code.slice(14)}`) : t('deviceLayouts.failed', { error: code });
  };
  const refresh = async () => {
    setLoading(true); setLoadError('');
    try { const value = await listLocalDeviceLayouts(); if (alive.current) setLayouts(value); }
    catch (cause) { if (alive.current) setLoadError(errorText(cause)); }
    finally { if (alive.current) setLoading(false); }
  };
  useEffect(() => {
    alive.current = true;
    void refresh();
    dialog.current?.showModal();
    return () => { alive.current = false; dialog.current?.close(); };
  }, []);
  const attempt = (action: () => void) => dirty
    ? setConfirmation({ text: t('deviceLayouts.unsaved'), action }) : action();
  const reset = (value?: DeviceLayout, writing = false) => {
    setEditing(value ?? null); setName(value?.name ?? '');
    setProfile(structuredClone(value?.profile ?? (writing ? writingDeviceKeyProfile() : defaultDeviceKeyProfile())));
    setKnob(value?.knobRotationAction ?? 'systemVolume'); setGroup('deviceCustomKeys');
    setDirty(writing); setMessage(''); setConfirmation(null);
    setPracticeRevision(value => value + 1);
  };
  const edit = (value: DeviceCustomKeyMapping) => {
    setProfile(current => ({ ...current, [actionGroup]: { ...current[actionGroup], [key]: value } }));
    setDirty(true); setMessage('');
  };
  const save = async () => {
    if (busy || loading || loadError || !name.trim()) return;
    setBusy(true); setMessage('');
    try {
      const value = await saveLocalDeviceLayout(editing?.id ?? null, editing?.revision ?? null, name, profile, knob);
      if (!alive.current) return;
      setLayouts(current => [...current.filter(layout => layout.id !== value.id), value]);
      setEditing(value); setProfile(value.profile); setName(value.name); setDirty(false); setConfirmation(null);
      setMessage(t('deviceLayouts.saved'));
    } catch (cause) { if (alive.current) setMessage(errorText(cause)); }
    finally { if (alive.current) setBusy(false); }
  };
  const remove = async (value: DeviceLayout) => {
    setBusy(true); setMessage(''); setConfirmation(null);
    try {
      await deleteLocalDeviceLayout(value.id, value.revision);
      if (!alive.current) return;
      setLayouts(current => current.filter(layout => layout.id !== value.id)); reset();
    } catch (cause) { if (alive.current) setMessage(errorText(cause)); }
    finally { if (alive.current) setBusy(false); }
  };
  return <dialog ref={dialog} className="ol-device-layout-dialog ol-thinscroll" aria-labelledby="device-layout-title"
    onCancel={event => { event.preventDefault(); if (!busy) attempt(onClose); }}>
    <div className="ol-device-layout-heading">
      <h2 id="device-layout-title">{t('deviceLayouts.title')}</h2>
      <Btn size="sm" variant="ghost" disabled={busy} onClick={() => attempt(onClose)}>{t('common.close')}</Btn>
    </div>
    <p className="ol-device-layout-hint">{t(practice ? 'devicePractice.intro' : 'deviceLayouts.description')}</p>
    <div className="ol-device-layout-mode" role="group" aria-label={t('devicePractice.mode')}>
      <button type="button" aria-pressed={!practice} disabled={busy} onClick={() => setPractice(false)}>{t('devicePractice.configure')}</button>
      <button type="button" aria-pressed={practice} disabled={busy} onClick={() => setPractice(true)}>{t('devicePractice.practice')}</button>
    </div>
    {!nativeDeviceLayoutsAvailable && <p role="note" className="ol-device-layout-hint">{t('deviceLayouts.previewOnly')}</p>}
    {confirmation && <div role="alert" className="ol-device-layout-confirm">
      <p>{confirmation.text}</p>
      <Btn size="sm" variant="ghost" onClick={() => setConfirmation(null)}>{t('deviceLayouts.keepEditing')}</Btn>{' '}
      <Btn size="sm" variant="ghost" onClick={confirmation.action}>{t('deviceLayouts.discardContinue')}</Btn>
    </div>}
    <fieldset disabled={busy} className="ol-device-layout-fields">
      <div hidden={practice}>
      <div className="ol-device-layout-toolbar">
        <label>{t('deviceLayouts.library')}
          <select aria-label={t('deviceLayouts.library')} value={editing?.id ?? ''} disabled={loading} onChange={event => {
            const value = layouts.find(layout => layout.id === event.target.value); attempt(() => reset(value));
          }}><option value="">{t('deviceLayouts.newLayout')}</option>{layouts.map(layout => <option key={layout.id} value={layout.id}>{layout.name}</option>)}</select>
        </label>
        <Btn size="sm" variant="ghost" onClick={() => attempt(() => reset())}>{t('deviceLayouts.newLayout')}</Btn>
        <Btn size="sm" variant="ghost" onClick={() => attempt(() => reset(undefined, true))}>{t('deviceLayouts.writingStarter')}</Btn>
      </div>
      {loading && <p role="status">{t('common.loading')}</p>}
      {loadError && <p role="alert">{loadError}{' '}<Btn size="sm" onClick={() => void refresh()}>{t('common.retry')}</Btn></p>}
      <label className="ol-device-layout-name">{t('deviceLayouts.name')}<input aria-label={t('deviceLayouts.name')} maxLength={80} value={name}
        placeholder={t('deviceLayouts.namePlaceholder')} onChange={event => { setName(event.target.value); setDirty(true); setMessage(''); }} /></label>
      <p className="ol-device-layout-hint">{t('deviceLayouts.pickKey')}</p>
      <div className="ol-device-layout-pad">
        {layoutKeyIds.map(id => <button type="button" key={id} aria-pressed={key === id} onClick={() => setKey(id)}>
          <strong>{id === 'knob' ? t('settings.deviceKeys.knob.title') : t('deviceLayouts.key', { number: id.slice(3) })}</strong>
          <span>{t(`settings.deviceKeys.actions.${profile[id === 'knob' ? 'deviceCustomKeys' : group][id].action}`)}</span>
        </button>)}
      </div>
      <div className="ol-device-layout-editor">
        <h3>{key === 'knob' ? t('settings.deviceKeys.knob.title') : t('deviceLayouts.key', { number: key.slice(3) })}</h3>
        {key !== 'knob' && <label>{t('deviceLayouts.gesture')}<select aria-label={t('deviceLayouts.gesture')} value={group} onChange={event => setGroup(event.target.value as Group)}>
          {layoutGestures.map((value, i) => <option key={value} value={value}>{t(`settings.deviceKeys.gestures.${['singleClick','doubleClick','longPress'][i]}`)}</option>)}
        </select></label>}
        <label>{t('deviceLayouts.action')}<select aria-label={t('deviceLayouts.action')} value={mapping.action} onChange={event => {
          const action = event.target.value as DeviceCustomKeyMapping['action'];
          edit({ ...mapping, action, shortcut: action === 'sendShortcut' ? mapping.shortcut ?? {primary:'F1',modifiers:[]} : mapping.shortcut });
        }}>
          {layoutActions.map(action => <option key={action} value={action}>{t(`settings.deviceKeys.actions.${action}`)}</option>)}
        </select></label>
        {mapping.action === 'pasteTemplate' && <label>{t('deviceLayouts.template')}<textarea rows={3} maxLength={2000} aria-label={t('deviceLayouts.template')} value={mapping.pasteTemplate} onChange={event => edit({...mapping,pasteTemplate:event.target.value})} /></label>}
        {mapping.action === 'openApp' && <label>{t('deviceLayouts.page')}<select aria-label={t('deviceLayouts.page')} value={mapping.appPage} onChange={event => edit({...mapping,appPage:event.target.value as DeviceCustomKeyMapping['appPage']})}>
          {layoutAppPages.map(page => <option value={page} key={page}>{t(`settings.deviceKeys.appPages.${page}`)}</option>)}
        </select></label>}
        {mapping.action === 'openExternalApp' && <label>{t('deviceLayouts.applicationPath')}<input aria-label={t('deviceLayouts.applicationPath')} maxLength={512} value={mapping.externalAppPath} onChange={event => edit({...mapping,externalAppPath:event.target.value})} /></label>}
        {mapping.action === 'sendShortcut' && <>
          <label>{t('deviceLayouts.primaryKey')}<select aria-label={t('deviceLayouts.primaryKey')} value={mapping.shortcut?.primary ?? 'F1'} onChange={event => edit({...mapping,shortcut:{primary:event.target.value,modifiers:mapping.shortcut?.modifiers ?? []}})}>
            {primaryKeys.map(primary => <option key={primary} value={primary}>{primary}</option>)}
          </select></label>
          <div className="ol-device-layout-modifiers">{['ctrl','shift','alt','super'].map(modifier => <label key={modifier}><input type="checkbox" checked={mapping.shortcut?.modifiers.includes(modifier) ?? false} onChange={event => edit({...mapping,shortcut:{primary:mapping.shortcut?.primary ?? 'F1',modifiers:event.target.checked ? [...(mapping.shortcut?.modifiers ?? []),modifier] : (mapping.shortcut?.modifiers ?? []).filter(value => value !== modifier)}})} />{modifier === 'super' ? 'Win / Command' : modifier}</label>)}</div>
        </>}
        {key === 'knob' && <>
          <label>{t('settings.deviceKeys.knob.gestures.rotate')}<select aria-label={t('settings.deviceKeys.knob.gestures.rotate')} value={knob} onChange={event => { setKnob(event.target.value as DeviceKnobRotationAction); setDirty(true); }}>
            {(['systemVolume','screenBrightness','disabled'] as const).map(value => <option key={value} value={value}>{t(`settings.deviceKeys.knob.actions.${value}`)}</option>)}
          </select></label>
          <p className="ol-device-layout-hint">{t('deviceLayouts.fixedKnob')}</p>
        </>}
      </div>
      <p className="ol-device-layout-hint">{t('deviceLayouts.requirements')}</p>
      {message && <p role="status">{message}</p>}
      <div className="ol-device-layout-toolbar">
        <Btn variant="blue" disabled={loading || !!loadError || !name.trim() || !dirty} onClick={() => void save()}>{t(busy ? 'deviceLayouts.saving' : 'deviceLayouts.save')}</Btn>
        {onUseLayout && <Btn variant="ghost" disabled={!editing || dirty} onClick={() => { if (editing) { onUseLayout(editing); onClose(); } }}>{t('deviceLayouts.loadIntoSettings')}</Btn>}
        {editing && <Btn size="sm" variant="ghost" onClick={() => setConfirmation({ text: t('deviceLayouts.deleteConfirm', {name:editing.name}), action: () => void remove(editing) })}>{t('deviceLayouts.delete')}</Btn>}
        {dirty && <span className="ol-device-layout-hint">{t('deviceLayouts.unsavedShort')}</span>}
      </div>
      </div>
      <div hidden={!practice}><DeviceLayoutPractice key={practiceRevision} profile={profile} /></div>
    </fieldset>
  </dialog>;
}
