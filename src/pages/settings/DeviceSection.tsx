import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ShortcutRecorder } from '../../components/ShortcutRecorder';
import { detectOS } from '../../components/WindowChrome';
import { SelectLite } from '../../components/ui/SelectLite';
import {
  cancelVoiceprintEnrollment,
  listSettingsDevices,
  getSelectedDeviceSettings,
  setSelectedDeviceSettings,
  setSelectedDeviceKeyProfile,
  getEmbeddedBleRuntimeStatus,
  getVoiceprintStatus,
  listInstalledApplications,
  deleteVoiceprint,
  startVoiceprintEnrollment,
  retryVoiceprintSave,
} from '../../lib/ipc';
import type {
  DeviceCustomKeyAction,
  DeviceCustomKeyAppPage,
  DeviceCustomKeyGesture,
  DeviceCustomKeyId,
  DeviceCustomKeys,
  DeviceKeyProfile,
  SelectedDeviceSettings,
  DeviceCustomKeyMapping,
  DeviceKnobRotationAction,
  DeviceSettingsSnapshot,
  DeviceSettingsUpdateRequest,
  SettingsDevice,
  InstalledApplication,
  PostDictationKey,
  ShortcutBinding,
  VoiceprintStatus,
} from '../../lib/types';
import { useHotkeySettings } from '../../state/HotkeySettingsContext';
import { Btn, Card } from '../_atoms';
import { FirmwareOtaPanel } from './FirmwareOtaPanel';
import { VoiceprintEnrollmentWizard } from './VoiceprintEnrollmentWizard';
import { inputStyle, SettingRow, Toggle } from './shared';
import type { EmbeddedBleProbeStatus } from '../../components/EmbeddedBleStatusPanel';
import {
  DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
  DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
  DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
  DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
} from '../../lib/deviceSettingsDefaults';
import { shouldRefreshDeviceSettingsOnFocus } from '../../lib/deviceSettingsRefresh';
import { DeviceSettingsDraft } from '../../lib/deviceSettingsDraft';
import { serializeDeviceSettingsSave } from '../../lib/deviceSettingsSaveQueue';
import { DeviceLayoutsDialog } from '../../components/DeviceLayoutsDialog';
import { defaultDeviceKeyProfile } from '../../lib/deviceKeyLayouts';

const DEVICE_KEYS = [
  { id: 'key1' },
  { id: 'key2' },
  { id: 'key3' },
  { id: 'key4' },
] as const satisfies ReadonlyArray<{ id: DeviceCustomKeyId }>;

type DeviceKeyMapKey =
  | 'deviceCustomKeys'
  | 'deviceCustomKeyDoubleClicks'
  | 'deviceCustomKeyLongPresses';
type DeviceKeyEdits = Partial<Record<DeviceKeyMapKey,Partial<Record<DeviceCustomKeyId,Partial<DeviceCustomKeyMapping>>>>>;

function mergeDeviceKeyEdits(base:DeviceKeyProfile, edits:DeviceKeyEdits):DeviceKeyProfile {
  const merged=structuredClone(base);
  for(const map of Object.keys(edits) as DeviceKeyMapKey[]) {
    for(const id of Object.keys(edits[map] ?? {}) as DeviceCustomKeyId[]) {
      merged[map][id]={...base[map][id],...edits[map]?.[id]};
    }
  }
  return merged;
}

// A completed request acknowledges only the fields it actually submitted.
// Layout application can add newer edits while a slow device save is pending.
function acknowledgeDeviceKeyEdits(current:DeviceKeyEdits, submitted:DeviceKeyEdits):DeviceKeyEdits {
  const remaining=structuredClone(current);
  for(const map of Object.keys(submitted) as DeviceKeyMapKey[]) {
    for(const id of Object.keys(submitted[map] ?? {}) as DeviceCustomKeyId[]) {
      const fields=remaining[map]?.[id];
      if(!fields)continue;
      for(const field of Object.keys(submitted[map]?.[id] ?? {}) as (keyof DeviceCustomKeyMapping)[]) {
        if(JSON.stringify(fields[field])===JSON.stringify(submitted[map]?.[id]?.[field]))delete fields[field];
      }
      if(Object.keys(fields).length===0)delete remaining[map]?.[id];
    }
    if(Object.keys(remaining[map] ?? {}).length===0)delete remaining[map];
  }
  return remaining;
}

const DEVICE_GESTURES: Array<{
  id: DeviceCustomKeyGesture;
  mapKey: DeviceKeyMapKey;
}> = [
  {
    id: 'singleClick',
    mapKey: 'deviceCustomKeys',
  },
  {
    id: 'doubleClick',
    mapKey: 'deviceCustomKeyDoubleClicks',
  },
  {
    id: 'longPress',
    mapKey: 'deviceCustomKeyLongPresses',
  },
];

const DEVICE_KEY_ACTIONS: DeviceCustomKeyAction[] = [
  'sendShortcut',
  'dictation',
  'openApp',
  'pasteShortcut',
  'openExternalApp',
  'copyShortcut',
  'undoShortcut',
  'pasteTemplate',
  'switchStyle',
  'translation',
  'selectionAsk',
  'disabled',
];

const DEVICE_KEY_APP_PAGES: DeviceCustomKeyAppPage[] = [
  'overview',
  'history',
  'vocab',
  'style',
  'translation',
  'selectionAsk',
  'settingsRecording',
  'settingsDevice',
  'settingsProviders',
  'settingsShortcuts',
  'settingsPermissions',
  'settingsLanguage',
  'settingsAdvanced',
];

const KNOB_ROTATION_ACTIONS: DeviceKnobRotationAction[] = ['systemVolume', 'screenBrightness', 'disabled'];

const KNOB_FIXED_ACTION_ROWS = [
  { gesture: 'doubleClick', action: 'bluetoothReset' },
  { gesture: 'longPress', action: 'powerOff' },
] as const;
const DEVICE_KEY_LABEL_WIDTH = 50;
const DEVICE_KEY_LABEL_GAP = 16;
const DEVICE_KEY_ROW_TEMPLATE = `${DEVICE_KEY_LABEL_WIDTH}px minmax(0, 1fr)`;
const DEVICE_KEY_CONTROL_GAP = 8;
const DEVICE_KEY_MAIN_CONTROL_WIDTH = 208;
const DEVICE_KEY_SECONDARY_CONTROL_WIDTH = 208;
const DEVICE_KEY_DETAIL_WIDTH = DEVICE_KEY_MAIN_CONTROL_WIDTH + DEVICE_KEY_CONTROL_GAP + DEVICE_KEY_SECONDARY_CONTROL_WIDTH;
const KNOB_CONTROL_WIDTH = 208;

const EXTERNAL_APP_MANUAL_VALUE = '__manual_external_app__';
const DEVICE_SETTINGS_REFRESH_MS = 8000;
const DEVICE_SETTINGS_READ_TIMEOUT_MS = 80_000;
const DEVICE_SETTINGS_WRITE_TIMEOUT_MS = 80_000;
const DEFAULT_BATTERY_AUTO_SHUTDOWN_MINUTES = 10;

const fallbackShortcut = (): ShortcutBinding => ({
  primary: 'F1',
  modifiers: [],
});

function isDeviceFallbackReservedPrimary(primary: string): boolean {
  const match = /^F(\d{1,2})$/i.exec(primary.trim());
  if (!match) return false;
  const keyNumber = Number(match[1]);
  return keyNumber >= 13 && keyNumber <= 24;
}

export function DeviceSection() {
  const [selectedDevice, setSelectedDevice] = useState<SettingsDevice | null>(null);
  const [selectedValue, setSelectedValue] = useState<SelectedDeviceSettings | null>(null);
  return <>
    <DeviceFirmwareSettingsCard onSelectedChange={device => { setSelectedDevice(device); setSelectedValue(null); }} onRead={setSelectedValue} />
    <SelectedDeviceKeysCard key={selectedDevice?.id ?? 'none'} device={selectedDevice} value={selectedValue} onRead={setSelectedValue} />
    <FirmwareOtaPanel supported={detectOS() === 'win'} />
  </>;
}
const emptyDeviceKeyProfile = defaultDeviceKeyProfile;
function snapshotKnob(value: SelectedDeviceSettings | null): DeviceKnobRotationAction {
  switch(value?.snapshot.knobRotationAction) { case 'screen_brightness': case 'screenBrightness': return 'screenBrightness';case 'disabled':return 'disabled';default:return 'systemVolume'; }
}
function SelectedDeviceKeysCard({device,value,onRead}:{device:SettingsDevice|null;value:SelectedDeviceSettings|null;onRead:(value:SelectedDeviceSettings)=>void}) {
  const { t } = useTranslation();
  const [profile,setProfile]=useState<DeviceKeyProfile>(value?.keys?.profile ?? emptyDeviceKeyProfile());
  const [knobRotationAction,setKnob]=useState<DeviceKnobRotationAction>(snapshotKnob(value));
  const [busy,setBusy]=useState(false);
  const [saving,setSaving]=useState(false);
  const [saveFailed,setSaveFailed]=useState(false);
  const [dirty,setDirty]=useState(false);
  const [message,setMessage]=useState('');
  const [layoutsOpen,setLayoutsOpen]=useState(false);
  const [installedApps,setInstalledApps]=useState<InstalledApplication[]>([]);
  const [installedAppsLoading,setInstalledAppsLoading]=useState(false);
  const autoOpenDeviceKeyActionMenu=import.meta.env.DEV && new URLSearchParams(window.location.search).get('openDeviceKeyActionMenu')==='1';
  const changes=useRef<DeviceKeyEdits>({});
  const knobChanged=useRef(false);
  const knobDraft=useRef(knobRotationAction);
  const saveInFlight=useRef(false);
  const generation=useRef(0);
  const mounted=useRef(true);
  const autosaveTimer=useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveLatest=useRef<() => Promise<void>>(async()=>{});
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;if(autosaveTimer.current!==null){clearTimeout(autosaveTimer.current);autosaveTimer.current=null;void saveLatest.current();}++generation.current;};},[]);
  useEffect(()=>{let cancelled=false;setInstalledAppsLoading(true);listInstalledApplications().then(v=>{if(!cancelled)setInstalledApps(v);}).catch(()=>{}).finally(()=>{if(!cancelled)setInstalledAppsLoading(false);});return()=>{cancelled=true;};},[]);
  const merge=(base:DeviceKeyProfile):DeviceKeyProfile=>mergeDeviceKeyEdits(base,changes.current);
  useEffect(()=>{if(value?.device.id!==device?.id)return;if(value?.keys)setProfile(merge(value.keys.profile));if(!knobChanged.current)setKnob(snapshotKnob(value));},[value]);
  const editKey=(map:DeviceKeyMapKey,id:DeviceCustomKeyId,mapping:DeviceCustomKeyMapping,commit=true,replace=false)=>{
    const previous=profile[map][id];
    const patch=replace ? mapping : Object.fromEntries(Object.entries(mapping).filter(([k,v])=>JSON.stringify(v)!==JSON.stringify(previous[k as keyof DeviceCustomKeyMapping])));
    if(Object.keys(patch).length===0)return;
    changes.current={...changes.current,[map]:{...changes.current[map],[id]:{...changes.current[map]?.[id],...patch}}};
    setProfile(p=>({...p,[map]:{...p[map],[id]:mapping}}));setDirty(true);
    setSaveFailed(false);
    setMessage(t('settings.device.autoSavePending','编辑完成后自动保存到设备'));
    if(commit)scheduleSave();
  };
  const updateKnobRotationAction=async(value:string)=>{knobChanged.current=true;knobDraft.current=value as DeviceKnobRotationAction;setKnob(knobDraft.current);setDirty(true);scheduleSave();};
  const restoreDefaultDeviceKeyboardOutput=()=>{const keys=emptyDeviceKeyProfile().deviceCustomKeys;for(const id of ['key1','key2','key3','key4','knob'] as const)editKey('deviceCustomKeys',id,keys[id],true,true);};
  const read=async()=>{
    if(!device || busy)return;const current=++generation.current;setBusy(true);setMessage('');
    try {const result=await getSelectedDeviceSettings(device);if(current!==generation.current)return;onRead(result);setMessage(result.keysError ?? (result.keys ? '' : t('settings.deviceKeys.firmwareRequired','这台设备的固件尚不支持独立保存按键，请先更新固件。')));}
    catch(e){if(current===generation.current)setMessage(String(e));}
    finally{if(current===generation.current)setBusy(false);}
  };
  const save=async()=>{
    if(!device || saveInFlight.current)return;
    if(!knobChanged.current && Object.keys(changes.current).length===0)return;
    saveInFlight.current=true;const current=++generation.current;
    if(mounted.current){setBusy(true);setSaving(true);setSaveFailed(false);setMessage('');}
    try {
      // Keep draining this component's original device even if it is unmounted
      // during the write. Newer layout edits must not be cleared by an old ACK
      // or silently abandoned when its timer fires while the request is busy.
      while(knobChanged.current || Object.keys(changes.current).length>0) {
        const pending=structuredClone(changes.current);const pendingKnob=knobChanged.current;const knobValue=knobDraft.current;
        const saved=await serializeDeviceSettingsSave(async()=>{
          const baseline=await getSelectedDeviceSettings(device);
          if(baseline.device.id!==device.id)throw new Error('Selected device identity changed; no settings written.');
          if(!baseline.keys)throw new Error(baseline.keysError ?? t('settings.deviceKeys.firmwareRequired','这台设备的固件尚不支持独立保存按键，请先更新固件。'));
          return setSelectedDeviceKeyProfile(device,baseline.revision,baseline.readToken,baseline.keys.revision,mergeDeviceKeyEdits(baseline.keys.profile,pending),pendingKnob ? knobValue : null);
        });
        changes.current=acknowledgeDeviceKeyEdits(changes.current,pending);
        if(pendingKnob && knobDraft.current===knobValue)knobChanged.current=false;
        if(mounted.current && current===generation.current) {
          onRead(saved);
          if(saved.keys)setProfile(mergeDeviceKeyEdits(saved.keys.profile,changes.current));
          setKnob(knobChanged.current ? knobDraft.current : snapshotKnob(saved));
        }
      }
      if(mounted.current && current===generation.current){setDirty(false);setMessage(t('settings.device.autoSaved','已保存到设备'));}
    }catch(e){if(mounted.current && current===generation.current){setSaveFailed(true);setMessage(t('settings.device.autoSaveFailed','保存失败，修改已保留：')+' '+String(e));}}
    finally{saveInFlight.current=false;if(mounted.current && current===generation.current){setBusy(false);setSaving(false);}}
  };
  saveLatest.current=save;
  function scheduleSave(){
    if(autosaveTimer.current!==null)clearTimeout(autosaveTimer.current);
    // Empty templates/paths are unfinished edits, never a request to save an invalid action.
    const staged=merge(profile);
    const incomplete=(Object.keys(changes.current) as DeviceKeyMapKey[]).some(map=>(Object.keys(changes.current[map] ?? {}) as DeviceCustomKeyId[]).some(id=>{
      const m=staged[map][id];return (m.action==='pasteTemplate'&&!m.pasteTemplate.trim()) || (m.action==='openExternalApp'&&!m.externalAppPath.trim());
    }));
    if(incomplete){autosaveTimer.current=null;return;}
    autosaveTimer.current=setTimeout(()=>{autosaveTimer.current=null;void saveLatest.current();},120);
  }
  return (
      <Card>
        <fieldset disabled={busy} onBlurCapture={event=>{if(event.target instanceof HTMLInputElement)scheduleSave();}} style={{border: 0, padding: 0, margin: 0, minWidth: 0}}>
        <div style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) auto', alignItems: 'start', gap: 12 }}>
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
              {t('settings.deviceKeys.title')}
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.35 }}>
              {t('settings.deviceKeys.description')}
            </div>
          </div>
          <Btn
            variant="ghost"
            size="sm"
            icon="refresh"
            onClick={() => restoreDefaultDeviceKeyboardOutput()} disabled={busy}
            style={{ height: 32, whiteSpace: 'nowrap', justifyContent: 'center' }}
          >
            {t('settings.deviceKeys.restoreKeyboardDefaults')}
          </Btn>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: 12 }}>
          {DEVICE_GESTURES.map(gesture => (
            <DeviceKeyGestureGroup
              key={gesture.id}
              gesture={gesture}
              keys={profile[gesture.mapKey]}
              installedApps={installedApps}
              installedAppsLoading={installedAppsLoading}
              autoOpenDeviceKeyActionMenu={autoOpenDeviceKeyActionMenu}
              onChange={async (id, mapping, commit) => editKey(gesture.mapKey, id, mapping, commit)}
            />
          ))}
        </div>

        <KnobActionsPanel
          knobClickMapping={profile.deviceCustomKeys.knob}
          knobRotationAction={knobRotationAction}
          installedApps={installedApps}
          installedAppsLoading={installedAppsLoading}
          onKnobClickMappingChange={async (mapping,commit) => editKey('deviceCustomKeys', 'knob', mapping,commit)}
          onKnobRotationActionChange={updateKnobRotationAction}
        />
        <div style={{fontSize: 12, color: 'var(--ol-ink-4)', marginTop: 12}}>{t('settings.deviceKeys.onboardAutoSave', '修改后自动保存到当前设备；换电脑仍保留。')}</div>
        {saving && <div role="status" style={{fontSize:12,marginTop:10}}>{t('settings.device.autoSaving','正在保存到设备…')}</div>}
        {message && <div role="status" style={{fontSize: 12, marginTop: 10}}>{message}</div>}
        <div style={{display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 12}}>
          <Btn variant="ghost" size="sm" onClick={() => setLayoutsOpen(true)}>{t('deviceLayouts.library')}</Btn>
          <Btn variant="ghost" size="sm" onClick={() => void read()} disabled={!device || busy}>{t('settings.device.readFromDevice', '读取')}</Btn>
          {dirty && saveFailed && <Btn variant="blue" size="sm" onClick={() => void save()} disabled={!device || busy}>{t('settings.device.retrySave','重试保存')}</Btn>}
        </div>
        </fieldset>
        {layoutsOpen && <DeviceLayoutsDialog initialProfile={profile} initialKnob={knobRotationAction}
          onClose={() => setLayoutsOpen(false)}
          onUseLayout={device ? layout => {
            for (const gesture of DEVICE_GESTURES) {
              for (const id of ['key1','key2','key3','key4','knob'] as const) editKey(gesture.mapKey,id,layout.profile[gesture.mapKey][id],true,true);
            }
            void updateKnobRotationAction(layout.knobRotationAction);
            setMessage(t('deviceLayouts.loadedDraft'));
          } : undefined} />}
      </Card>

  );
}

function DeviceFirmwareSettingsCard({onSelectedChange,onRead}:{onSelectedChange:(device:SettingsDevice|null)=>void;onRead:(value:SelectedDeviceSettings)=>void}) {
  const { t } = useTranslation();
  const { prefs, updatePrefs: savePrefs } = useHotkeySettings();
  const [snapshot, setSnapshot] = useState<DeviceSettingsSnapshot | null>(null);
  const [devices, setDevices] = useState<SettingsDevice[]>([]);
  const [transport, setTransport] = useState<'usb' | 'ble'>('usb');
  const [selectedDevice, setSelectedDevice] = useState<SettingsDevice | null>(null);
  const [listing, setListing] = useState(false);
  const listGeneration = useRef(0);
  // Invalidates late reads after switching device or unmounting the page.
  const requestGeneration = useRef(0);
  const draft = useRef(new DeviceSettingsDraft());
  const [form, setForm] = useState<DeviceSettingsUpdateRequest>({
    statusLedBrightnessPercent: DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT,
    keyLedBrightnessPercent: DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT,
    knobLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
    edgeLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT,
    pluggedLowPowerIdleMinutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES,
    batteryLowPowerIdleMinutes: 1,
    pluggedLowPowerEnabled: DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED,
    voiceAutoStartEnabled: true,
    voiceAutoStopEnabled: true,
    pluggedAutoShutdownMinutes: 0,
    batteryAutoShutdownMinutes: DEFAULT_BATTERY_AUTO_SHUTDOWN_MINUTES,
    bleName: '',
  });
  const editForm = (patch: Partial<DeviceSettingsUpdateRequest>) => {
    draft.current.edit(patch);
    setForm(current => ({ ...current, ...patch }));
    setMessage(t('settings.device.autoSavePending','编辑完成后自动保存到设备'));
  };
  const [status, setStatus] = useState<'idle' | 'loading' | 'saving' | 'saved' | 'error'>('loading');
  const [message, setMessage] = useState('');
  const settingsMounted=useRef(true);
  const settingsSaveInFlight=useRef(false);
  useEffect(()=>{settingsMounted.current=true;return()=>{settingsMounted.current=false;};},[]);
  const [voiceprint, setVoiceprint] = useState<VoiceprintStatus | null>(null);
  const [voiceprintBusy, setVoiceprintBusy] = useState(false);
  const [voiceprintWizardOpen, setVoiceprintWizardOpen] = useState(false);
  const [voiceprintEnrollmentAttempted, setVoiceprintEnrollmentAttempted] = useState(false);
  const [wakePhraseDraft, setWakePhraseDraft] = useState('');
  const [wakePhraseBusy, setWakePhraseBusy] = useState(false);
  const [wakePhraseError, setWakePhraseError] = useState<string | null>(null);
  const voiceprintCaptureActive = ['preparing', 'armed', 'capturing', 'processing']
    .includes(voiceprint?.state ?? '');

  const refreshVoiceprint = async () => {
    try {
      setVoiceprint(await getVoiceprintStatus());
    } catch (error) {
      setVoiceprint(current => current ? {
        ...current,
        state: 'error',
        error: error instanceof Error ? error.message : String(error),
      } : null);
    }
  };

  useEffect(() => {
    void refreshVoiceprint();
    const timer = window.setInterval(
      () => void refreshVoiceprint(),
      voiceprintCaptureActive ? 350 : 1200,
    );
    return () => window.clearInterval(timer);
  }, [voiceprintCaptureActive]);

  useEffect(() => {
    if (prefs) {
      setWakePhraseDraft(prefs.voiceWakePhrase);
    }
  }, [prefs?.voiceWakePhrase]);

  const commitWakePhrase = async () => {
    if (!prefs || wakePhraseBusy || voiceprintCaptureActive) return;
    const normalizedPhrase = wakePhraseDraft.replace(/\s/g, '');
    setWakePhraseBusy(true);
    setWakePhraseError(null);
    try {
      await savePrefs(current => ({
        ...current,
        voiceWakePhrase: normalizedPhrase,
      }));
      await refreshVoiceprint();
    } catch (error) {
      setWakePhraseError(error instanceof Error ? error.message : String(error));
    } finally {
      setWakePhraseBusy(false);
    }
  };

  const enrollVoiceprint = async () => {
    setVoiceprintEnrollmentAttempted(true);
    setVoiceprintBusy(true);
    try {
      setVoiceprint(await startVoiceprintEnrollment());
    } catch (error) {
      setVoiceprint(current => current ? {
        ...current,
        state: 'error',
        error: error instanceof Error ? error.message : String(error),
      } : null);
    } finally {
      setVoiceprintBusy(false);
    }
  };

  const retryVoiceprintStorage = async () => {
    setVoiceprintBusy(true);
    try {
      setVoiceprint(await retryVoiceprintSave());
    } catch {
      await refreshVoiceprint();
    } finally {
      setVoiceprintBusy(false);
    }
  };

  const cancelVoiceprintCapture = async () => {
    setVoiceprintBusy(true);
    try {
      setVoiceprint(await cancelVoiceprintEnrollment());
      setVoiceprintWizardOpen(false);
      setVoiceprintEnrollmentAttempted(false);
    } catch (error) {
      setVoiceprint(current => current ? {
        ...current,
        state: 'error',
        error: error instanceof Error ? error.message : String(error),
      } : null);
    } finally {
      setVoiceprintBusy(false);
    }
  };

  const clearVoiceprint = async () => {
    setVoiceprintBusy(true);
    try {
      setVoiceprint(await deleteVoiceprint());
    } finally {
      setVoiceprintBusy(false);
    }
  };

  const refresh = async (device = selectedDevice) => {
    if (!device) return;
    const generation = ++requestGeneration.current;
    setStatus(previous => (previous === 'saving' ? previous : 'loading'));
    setMessage('');
    try {
      const value = await withTimeout(
        getSelectedDeviceSettings(device),
        DEVICE_SETTINGS_READ_TIMEOUT_MS,
        t('settings.device.readTimeout', '读取超时，请确认设备仍连接后重试。'),
      );
      if (generation !== requestGeneration.current || value.device.id !== device.id) return;
      setSnapshot(value.snapshot);
      onRead(value);
      if (value.snapshot.source !== 'firmware') {
        setStatus('error');
        setMessage(t('settings.device.readRequired', '未能读取设备当前设置；保留原有内容，重新读取成功后才能写入。'));
        return;
      }
      setForm(draft.current.merge(snapshotToForm(value.snapshot)));
      setStatus('idle');
    } catch (error) {
      if (generation !== requestGeneration.current) return;
      setStatus('error');
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const chooseDevice = (device: SettingsDevice | null) => {
    ++requestGeneration.current;
    setSelectedDevice(device);
    onSelectedChange(device);
    setSnapshot(null);
    draft.current = new DeviceSettingsDraft();
    // A new device must never inherit another board's draft. Defaults are
    // display-only until readback; only explicitly edited fields are saved.
    setForm({ statusLedBrightnessPercent: DEFAULT_DEVICE_STATUS_LED_BRIGHTNESS_PERCENT, keyLedBrightnessPercent: DEFAULT_DEVICE_KEY_LED_BRIGHTNESS_PERCENT, knobLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT, edgeLedBrightnessPercent: DEFAULT_DEVICE_LED_ZONE_BRIGHTNESS_PERCENT, pluggedLowPowerIdleMinutes: DEFAULT_DEVICE_PLUGGED_LOW_POWER_IDLE_MINUTES, batteryLowPowerIdleMinutes: 1, pluggedLowPowerEnabled: DEFAULT_DEVICE_PLUGGED_LOW_POWER_ENABLED, voiceAutoStartEnabled: true, voiceAutoStopEnabled: true, pluggedAutoShutdownMinutes: 0, batteryAutoShutdownMinutes: DEFAULT_BATTERY_AUTO_SHUTDOWN_MINUTES, bleName: '' });
    setMessage('');
    setStatus('idle');
    if (device) { setTransport(device.transport); void refresh(device); }
  };
  const refreshDevices = async () => {
    const generation = ++listGeneration.current;
    setListing(true);
    try {
      const available = await listSettingsDevices();
      if (generation !== listGeneration.current) return;
      setDevices(available);
      const current = selectedDevice && available.find(device => device.id === selectedDevice.id);
      if (current) setSelectedDevice(current);
      else if (selectedDevice) chooseDevice(null);
      else chooseDevice(available.find(device => device.transport === transport) ?? available[0] ?? null);
    } catch (error) {
      if (generation !== listGeneration.current) return;
      setMessage(error instanceof Error ? error.message : String(error));
      setStatus('error');
    } finally { if (generation === listGeneration.current) setListing(false); }
  };
  useEffect(() => {
    void refreshDevices();
    return () => { ++requestGeneration.current; ++listGeneration.current; };
  }, []);

  useEffect(() => {
    const refreshAuthoritativeDeviceState = () => {
      if (!shouldRefreshDeviceSettingsOnFocus(
        status,
        snapshot,
        form,
        snapshot ? snapshotToForm(snapshot) : null,
      )) return;
      void refresh();
    };
    window.addEventListener('focus', refreshAuthoritativeDeviceState);
    return () => window.removeEventListener('focus', refreshAuthoritativeDeviceState);
  }, [form, snapshot, status]);

  const validationError = snapshot || draft.current.has('bleName') ? validateDeviceSettingsForm({ ...form, bleName: form.bleName || (draft.current.has('bleName') ? '' : 'listener') }, t) : null;
  const writeDisabled = listing || !selectedDevice || status === 'loading' || status === 'saving' || !!validationError;
  const readDisabled = listing || !selectedDevice || status === 'loading' || status === 'saving';
  // Read failures and a missing device must not disable local draft editing.
  const controlsDisabled = status === 'saving';
  const busyText = status === 'loading'
    ? t('settings.device.readingDetail', '正在读取设备设置...')
    : status === 'saving'
      ? t('settings.device.writingDetail', '正在写入设备设置...')
      : '';

  const persistForm = async () => {
    // Explicit editing may finish before the initial device read. Do not drop
    // that intent because the display is still loading: this save obtains its
    // own fresh baseline and invalidates the earlier display-only read.
    if (!selectedDevice || settingsSaveInFlight.current || draft.current.empty) return;
    settingsSaveInFlight.current=true;
    const device = selectedDevice;
    const generation = ++requestGeneration.current;
    const pendingDraft = draft.current.clone();
    setStatus('saving');
    setMessage('');
    try {
      // Read this one target immediately before writing. A draft does not need
      // a previous successful read, and cached/default values are never sent.
      const value=await serializeDeviceSettingsSave(async()=>{
        const baseline = await withTimeout(getSelectedDeviceSettings(device), DEVICE_SETTINGS_READ_TIMEOUT_MS,
          t('settings.device.readTimeout', '读取超时，请确认设备仍连接后重试。'));
        if (baseline.device.id !== device.id) throw new Error('Selected device identity changed; no settings written.');
        const submittedForm = pendingDraft.merge(snapshotToForm(baseline.snapshot));
        const invalid = validateDeviceSettingsForm(submittedForm, t);
        if (invalid) throw new Error(invalid);
        return withTimeout(
          setSelectedDeviceSettings(device, baseline.revision, baseline.readToken, submittedForm),
          DEVICE_SETTINGS_WRITE_TIMEOUT_MS,
          t('settings.device.writeTimeout', '写入超时，请确认设备仍连接后重试。'),
        );
      });
      if (!settingsMounted.current || generation !== requestGeneration.current || value.device.id !== device.id) return;
      setSnapshot(value.snapshot);
      onRead(value);
      draft.current = new DeviceSettingsDraft();
      setForm(snapshotToForm(value.snapshot));
      setStatus('saved');
      setMessage(t('settings.device.autoSaved','已保存到设备'));
      window.setTimeout(() => setStatus(current => (current === 'saved' ? 'idle' : current)), 1800);
    } catch (error) {
      if (!settingsMounted.current || generation !== requestGeneration.current) return;
      setStatus('error');
      setMessage(t('settings.device.autoSaveFailed','保存失败，修改已保留：')+' '+(error instanceof Error ? error.message : String(error)));
    } finally {
      settingsSaveInFlight.current=false;
    }
  };
  const save = async () => persistForm();
  const saveVoiceAutomation = (
    patch: Partial<Pick<DeviceSettingsUpdateRequest, 'voiceAutoStartEnabled' | 'voiceAutoStopEnabled'>>,
  ) => {
    editForm(patch);
    void persistForm();
  };
  const handleSettingsKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey || event.nativeEvent.isComposing) {
      return;
    }
    const target = event.target;
    if (!(target instanceof HTMLInputElement)) {
      return;
    }
    event.preventDefault();
    target.blur();
  };
  const detailText = formatDeviceSnapshotDetail(snapshot, t);
  const footerText = validationError || message || formatDeviceSnapshotFooter(snapshot, t);

  return (
    <Card className="ol-device-settings-card" style={{ padding: 20 }}>
      <div onKeyDown={handleSettingsKeyDown}
        onBlur={event=>{if(event.target instanceof HTMLInputElement)void persistForm();}}
        onPointerUp={event=>{if(event.target instanceof HTMLInputElement && event.target.type==='range')void persistForm();}}>
      <div className="ol-device-settings-header" style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) auto', alignItems: 'center', gap: 12, marginBottom: 12 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 15, fontWeight: 700 }}>
            {t('settings.device.configTitle', '设备设置')}
          </div>
        </div>
        <div className="ol-device-settings-toolbar" style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', justifyContent: 'center' }}>
          <div className="ol-device-readwrite-row" style={{ display: 'flex', gap: 8 }}>
            <Btn variant="ghost" size="sm" icon={status === 'loading' ? undefined : 'refresh'} onClick={() => void refresh()} disabled={readDisabled} style={{ height: 34, justifyContent: 'center', minWidth: 0, whiteSpace: 'nowrap' }}>
              {status === 'loading' && <span className="ol-device-button-spinner" aria-hidden="true" />}
              {status === 'loading'
                ? t('settings.device.reading', '读取中')
                : t('settings.device.readFromDevice', '读取')}
            </Btn>
            {status==='error' && !draft.current.empty && <Btn variant="blue" size="sm" disabled={writeDisabled} onClick={() => void save()} style={{ height: 34, whiteSpace: 'nowrap' }}>{t('settings.device.retrySave','重试保存')}</Btn>}
          </div>
        </div>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))', gap: 10, padding: '14px 0', borderTop: '1px solid var(--ol-line)' }}>
        <label style={{ display: 'grid', gap: 6, fontSize: 12 }}>
          {t('settings.device.connectionMethod', '连接方式')}
          <SelectLite value={transport} disabled={status === 'saving' || listing}
            ariaLabel={t('settings.device.connectionMethod', '连接方式')}
            options={[{ value: 'usb', label: t('settings.device.usbMethod', 'USB 有线') }, { value: 'ble', label: t('settings.device.bleMethod', '蓝牙无线') }]}
            onChange={method => { const next = method as 'usb' | 'ble'; setTransport(next); chooseDevice(devices.find(device => device.transport === next) ?? null); }} />
        </label>
        <label style={{ display: 'grid', gap: 6, fontSize: 12 }}>
          {t('settings.device.selectedDevice', '设置设备')}
          <SelectLite value={selectedDevice?.id ?? ''} disabled={status === 'saving' || listing}
            ariaLabel={t('settings.device.selectedDevice', '设置设备')}
            placeholder={t('settings.device.noSelectedDevices', '未发现设备，请连接后刷新')}
            options={devices.filter(device => device.transport === transport).map(device => ({ value: device.id, label: device.label }))}
            onChange={id => chooseDevice(devices.find(device => device.id === id) ?? null)} />
        </label>
        <Btn variant="ghost" size="sm" icon="refresh" disabled={status === 'saving' || listing} onClick={() => void refreshDevices()} style={{ alignSelf: 'end', justifySelf: 'start' }}>
          {t('settings.device.refreshDevices', '刷新设备')}
        </Btn>
      </div>

      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginBottom: 10 }}>
        {t('settings.device.targetAutoSaveScope', '修改完成后自动保存到所选设备。USB 与蓝牙读取同一份设备配置，换电脑仍保留。')}
      </div>

      {busyText && (
        <div className="ol-device-settings-busy" role="status" aria-live="polite">
          <span className="ol-device-busy-spinner" aria-hidden="true" />
          <span>{busyText}</span>
        </div>
      )}

      <DeviceSettingsPanel
        title={t('settings.device.bleNameLabel', '蓝牙名称')}
        desc={t('settings.device.bleNameDesc', '1-29 个可见 ASCII 字符；部分 Windows 设备名变更需要重连或重新配对后才显示。')}
      >
        <div className="ol-device-ble-name-card">
          <input
            value={form.bleName}
            maxLength={29}
            disabled={controlsDisabled}
            placeholder="listener"
            onChange={event => editForm({ bleName: event.target.value })}
            style={{ ...inputStyle, flex: '0 1 320px', maxWidth: 320 }}
          />
        </div>
      </DeviceSettingsPanel>

      {detailText && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 8 }}>
          {detailText}
        </div>
      )}

      <DeviceSettingsPanel
        title={t('settings.device.powerTimingTitle', '电源时间')}
        desc={t('settings.device.powerTimingDesc', '插电和电池模式可分别设置低功耗等待与自动关机。')}
      >
        <div className="ol-device-power-timing-grid" style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(260px, 1fr))', gap: 10 }}>
          <PowerModeTimingGroup
            title={t('settings.device.pluggedModeTitle', '插电模式')}
            desc={t('settings.device.pluggedModeDesc', 'USB、充电或外部供电时使用。')}
            lowPowerLabel={t('settings.device.pluggedLowPowerIdleLabel', '进入低功耗')}
            lowPowerDesc={t('settings.device.pluggedLowPowerIdleDesc', '无操作多久后进入低功耗空闲。')}
            lowPowerValue={form.pluggedLowPowerIdleMinutes}
            disabled={controlsDisabled}
            onLowPowerChange={value => editForm({ pluggedLowPowerIdleMinutes: value, pluggedLowPowerEnabled: value > 0 })}
            t={t}
          />
          <PowerModeTimingGroup
            title={t('settings.device.batteryModeTitle', '电池模式')}
            desc={t('settings.device.batteryModeDesc', '拔电后使用；低功耗会灭灯。')}
            lowPowerLabel={t('settings.device.batteryLowPowerIdleLabel', '进入低功耗')}
            lowPowerDesc={t('settings.device.batteryLowPowerIdleDesc', '无操作多久进入低功耗空闲。')}
            lowPowerValue={form.batteryLowPowerIdleMinutes}
            autoShutdownLabel={t('settings.device.batteryAutoShutdownLabel', '自动关机')}
            autoShutdownDesc={t('settings.device.batteryAutoShutdownDesc', '电池供电且长时间空闲时自动关机。')}
            autoShutdownValue={form.batteryAutoShutdownMinutes}
            disabled={controlsDisabled}
            onLowPowerChange={value => editForm({ batteryLowPowerIdleMinutes: value })}
            onAutoShutdownChange={value => editForm({ batteryAutoShutdownMinutes: value })}
            t={t}
          />
        </div>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 8, lineHeight: 1.4 }}>
          {t('settings.device.zeroMeansOff', '0 = 关闭')}
        </div>
      </DeviceSettingsPanel>

      {prefs && (
        <DeviceSettingsPanel
          title={t('settings.recording.postActionsTitle')}
          desc={t('settings.recording.postActionsDesc')}
        >
          <div className="ol-recording-automation-settings">
            <SettingRow
              label={t('settings.recording.voiceAutoStartLabel')}
              desc={t('settings.recording.voiceAutoStartDesc')}
            >
              <Toggle
                on={form.voiceAutoStartEnabled}
                onToggle={voiceAutoStartEnabled =>
                  saveVoiceAutomation({ voiceAutoStartEnabled })}
                disabled={controlsDisabled}
              />
            </SettingRow>
            <SettingRow
              label={t('settings.recording.wakePhraseLabel', '唤醒词')}
              desc={t('settings.recording.wakePhraseDesc', '可自定义中文唤醒词；修改后需要重新录制声纹。')}
            >
              <div className="ol-wake-phrase-control">
                <div className="ol-wake-phrase-editor">
                  <input
                    value={wakePhraseDraft}
                    onChange={event => {
                      setWakePhraseDraft(event.target.value);
                      setWakePhraseError(null);
                    }}
                    onKeyDown={event => {
                      if (event.key === 'Enter') {
                        event.preventDefault();
                        void commitWakePhrase();
                      }
                    }}
                    maxLength={16}
                    disabled={controlsDisabled || wakePhraseBusy || voiceprintCaptureActive}
                    aria-label={t('settings.recording.wakePhraseLabel', '唤醒词')}
                    style={{ ...inputStyle, width: '100%', boxSizing: 'border-box' }}
                  />
                  <Btn
                    variant="blue"
                    size="sm"
                    icon="check"
                    disabled={
                      controlsDisabled
                      || wakePhraseBusy
                      || voiceprintCaptureActive
                      || wakePhraseDraft.replace(/\s/g, '') === prefs.voiceWakePhrase
                    }
                    onClick={() => void commitWakePhrase()}
                    style={{ justifyContent: 'center', whiteSpace: 'nowrap' }}
                  >
                    {wakePhraseBusy
                      ? t('settings.recording.wakePhraseApplying', '应用中')
                      : t('settings.recording.wakePhraseApply', '应用')}
                  </Btn>
                </div>
                <div className="ol-voiceprint-actions">
                  <Btn
                    variant={voiceprint?.enrolled ? 'ghost' : 'blue'}
                    size="sm"
                    icon="mic"
                    disabled={voiceprintBusy || !voiceprint?.available || voiceprintCaptureActive || wakePhraseDraft.replace(/\s/g, '') !== prefs.voiceWakePhrase}
                    onClick={() => {
                      setVoiceprintEnrollmentAttempted(false);
                      setVoiceprintWizardOpen(true);
                    }}
                    style={{ justifyContent: 'center' }}
                  >
                    {voiceprint?.state === 'preparing'
                      ? t('settings.recording.voiceprintPreparing', '准备中')
                      : voiceprint?.state === 'capturing' || voiceprint?.state === 'armed'
                        ? t('settings.recording.voiceprintCapturing', { phrase: prefs.voiceWakePhrase })
                        : voiceprint?.state === 'processing'
                          ? t('settings.recording.voiceprintProcessing', '校验中')
                          : voiceprint?.enrolled || voiceprint?.requiresReenrollment
                            ? t('settings.recording.voiceprintRedo', '重新录制')
                            : t('settings.recording.voiceprintEnroll', '录制声纹')}
                  </Btn>
                  {(voiceprint?.enrolled || voiceprint?.requiresReenrollment) && (
                    <Btn
                      variant="ghost"
                      size="sm"
                      icon="trash"
                      disabled={voiceprintBusy}
                      onClick={() => void clearVoiceprint()}
                      style={{ justifyContent: 'center' }}
                    >
                      {t('settings.recording.voiceprintDelete', '删除')}
                    </Btn>
                  )}
                </div>
                <div className={`ol-voiceprint-status${voiceprint?.requiresReenrollment ? ' is-warning' : ''}`}>
                  {voiceprint?.requiresReenrollment
                    ? t('settings.recording.voiceprintReenrollDesc', '声纹格式或唤醒词已更新，请重新录制。重新录制前，任何人说对唤醒词都可以启动。')
                    : voiceprint?.enrolled
                      ? t('settings.recording.voiceprintReadyDesc')
                      : t('settings.recording.voiceprintOpenGateDesc')}
                </div>
                {!voiceprint?.enrolled && !voiceprint?.requiresReenrollment && (
                  <div className="ol-voiceprint-status" style={{ opacity: 0.85 }}>
                    {t('settings.recording.voiceprintNoisyHint')}
                  </div>
                )}
                {wakePhraseError && (
                  <div className="ol-voiceprint-status is-error">{wakePhraseError}</div>
                )}
              </div>
            </SettingRow>
            {voiceprint?.error && (
              <div style={{ color: 'var(--ol-err)', fontSize: 11.5, lineHeight: 1.4 }}>
                {voiceprint.error}
              </div>
            )}
            <SettingRow
              label={t('settings.recording.voiceAutoStopLabel')}
              desc={t('settings.recording.voiceAutoStopDesc')}
            >
              <Toggle
                on={form.voiceAutoStopEnabled}
                onToggle={voiceAutoStopEnabled =>
                  saveVoiceAutomation({ voiceAutoStopEnabled })}
                disabled={controlsDisabled}
              />
            </SettingRow>
            <SettingRow
              label={t('settings.recording.removeFillerWordsLabel')}
              desc={t('settings.recording.removeFillerWordsDesc')}
            >
              <Toggle
                on={prefs.removeFillerWords}
                onToggle={removeFillerWords =>
                  savePrefs(current => ({ ...current, removeFillerWords }))}
              />
            </SettingRow>
            <SettingRow
              label={t('settings.recording.copyDictationToClipboardLabel')}
              desc={t('settings.recording.copyDictationToClipboardDesc')}
            >
              <Toggle
                on={prefs.copyDictationToClipboard}
                onToggle={copyDictationToClipboard =>
                  savePrefs(current => ({ ...current, copyDictationToClipboard }))}
              />
            </SettingRow>
            <SettingRow
              label={t('settings.recording.sendKeyAfterDictationLabel')}
              desc={t('settings.recording.sendKeyAfterDictationDesc')}
            >
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <Toggle
                  on={prefs.sendKeyAfterDictation}
                  onToggle={sendKeyAfterDictation =>
                    savePrefs(current => ({ ...current, sendKeyAfterDictation }))}
                />
                <SelectLite
                  value={prefs.postDictationKey}
                  onChange={postDictationKey =>
                    savePrefs(current => ({
                      ...current,
                      postDictationKey: postDictationKey as PostDictationKey,
                    }))}
                  options={[
                    { value: 'enter', label: t('settings.recording.postDictationKeyEnter') },
                    { value: 'ctrlEnter', label: t('settings.recording.postDictationKeyCtrlEnter') },
                  ]}
                  ariaLabel={t('settings.recording.postDictationKeyLabel')}
                  style={{ ...inputStyle, width: 132 }}
                  disabled={!prefs.sendKeyAfterDictation}
                />
              </div>
            </SettingRow>
          </div>
        </DeviceSettingsPanel>
      )}

      <DeviceLedBrightnessGroup
        supported={snapshot?.ledZoneBrightnessSupported ?? true}
        disabled={controlsDisabled}
        values={form}
        onChange={(field, value) => editForm({ [field]: value })}
        t={t}
      />

      {footerText && (
        <div style={{ fontSize: 11.5, color: validationError || status === 'error' ? 'var(--ol-err)' : status === 'saved' ? 'var(--ol-ok)' : 'var(--ol-ink-4)', lineHeight: 1.45, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
          {footerText}
        </div>
      )}
      {prefs && (
        <VoiceprintEnrollmentWizard
          open={voiceprintWizardOpen}
          attempted={voiceprintEnrollmentAttempted}
          busy={voiceprintBusy}
          phrase={prefs.voiceWakePhrase}
          status={voiceprint}
          onStart={() => void enrollVoiceprint()}
          onRetrySave={() => void retryVoiceprintStorage()}
          onCancel={() => void cancelVoiceprintCapture()}
          onClose={() => {
            setVoiceprintWizardOpen(false);
            setVoiceprintEnrollmentAttempted(false);
          }}
        />
      )}
      </div>
    </Card>
  );
}

function DeviceSettingsPanel({
  title,
  desc,
  children,
}: {
  title?: string;
  desc?: string;
  children: ReactNode;
}) {
  return (
    <section className="ol-device-settings-panel">
      {title && (
        <div className="ol-device-settings-panel-header">
          <div style={{ minWidth: 0 }}>
            <div style={{ fontSize: 12.5, fontWeight: 700 }}>{title}</div>
            {desc && (
              <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.45 }}>{desc}</div>
            )}
          </div>
        </div>
      )}
      {children}
    </section>
  );
}

function PowerModeTimingGroup({
  title,
  desc,
  lowPowerLabel,
  lowPowerDesc,
  lowPowerValue,
  autoShutdownLabel,
  autoShutdownDesc,
  autoShutdownValue,
  disabled,
  onLowPowerChange,
  onAutoShutdownChange,
  t,
}: {
  title: string;
  desc: string;
  lowPowerLabel: string;
  lowPowerDesc: string;
  lowPowerValue: number;
  autoShutdownLabel?: string;
  autoShutdownDesc?: string;
  autoShutdownValue?: number;
  disabled: boolean;
  onLowPowerChange: (value: number) => void;
  onAutoShutdownChange?: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="ol-device-power-mode-card">
      <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 10, marginBottom: 8 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 12.5, fontWeight: 700 }}>{title}</div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.45 }}>{desc}</div>
        </div>
      </div>
      <TimingControl
        label={lowPowerLabel}
        desc={lowPowerDesc}
        value={lowPowerValue}
        min={0}
        disabled={disabled}
        onChange={onLowPowerChange}
        t={t}
      />
      {autoShutdownLabel && autoShutdownDesc && typeof autoShutdownValue === 'number' && onAutoShutdownChange && (
        <TimingControl
          label={autoShutdownLabel}
          desc={autoShutdownDesc}
          value={autoShutdownValue}
          min={0}
          disabled={disabled}
          onChange={onAutoShutdownChange}
          t={t}
        />
      )}
    </div>
  );
}

function TimingControl({
  label,
  desc,
  value,
  min,
  disabled,
  onChange,
  t,
}: {
  label: string;
  desc: string;
  value: number;
  min: 0 | 1;
  disabled: boolean;
  onChange: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <div className="ol-device-timing-control" style={{ display: 'grid', gridTemplateColumns: 'minmax(0, 1fr) minmax(132px, 0.72fr)', gap: 12, alignItems: 'center', padding: '9px 0', borderTop: '0.5px solid var(--ol-line-soft)' }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontSize: 12.5, fontWeight: 600 }}>{label}</div>
        <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 2, lineHeight: 1.4 }}>{desc}</div>
      </div>
      <MinuteInput value={value} min={min} disabled={disabled} onChange={onChange} t={t} />
    </div>
  );
}

function MinuteInput({
  value,
  min,
  disabled,
  onChange,
  t,
}: {
  value: number;
  min: 0 | 1;
  disabled: boolean;
  onChange: (value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  const [draft, setDraft] = useState(() => String(value));
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) {
      setDraft(String(value));
    }
  }, [editing, value]);

  const commit = (raw: string, fallback: number) => {
    const trimmed = raw.trim();
    if (trimmed.length === 0) {
      setDraft(String(fallback));
      return;
    }
    const next = Number(trimmed);
    if (!Number.isFinite(next)) {
      setDraft(String(fallback));
      return;
    }
    const clamped = clampMinuteValue(next, min);
    onChange(clamped);
    setDraft(String(clamped));
  };

  const updateDraft = (raw: string) => {
    setDraft(raw);
    const trimmed = raw.trim();
    if (trimmed.length === 0) {
      return;
    }
    const next = Number(trimmed);
    if (!Number.isFinite(next)) {
      return;
    }
    const clamped = clampMinuteValue(next, min);
    if (clamped !== value) {
      onChange(clamped);
    }
  };

  return (
    <div className="ol-device-minute-input">
      <input
        type="number"
        inputMode="numeric"
        step={1}
        min={min}
        max={1440}
        value={draft}
        disabled={disabled}
        onFocus={() => setEditing(true)}
        onChange={event => {
          updateDraft(event.target.value);
        }}
        onKeyDown={event => {
          if (event.key === 'Enter') {
            event.currentTarget.blur();
          } else if (event.key === 'Escape') {
            setDraft(String(value));
            setEditing(false);
            event.currentTarget.blur();
          }
        }}
        onBlur={event => {
          setEditing(false);
          commit(event.target.value, value);
        }}
        onWheel={event => {
          event.currentTarget.blur();
        }}
        style={{ ...inputStyle, flex: '0 1 96px', maxWidth: 120 }}
      />
      <span style={{ fontSize: 12, color: 'var(--ol-ink-4)' }}>
        {t('settings.device.minutes', '分钟')}
      </span>
    </div>
  );
}

type LedBrightnessField =
  | 'statusLedBrightnessPercent'
  | 'keyLedBrightnessPercent'
  | 'knobLedBrightnessPercent'
  | 'edgeLedBrightnessPercent';

function DeviceLedBrightnessGroup({
  supported,
  disabled,
  values,
  onChange,
  t,
}: {
  supported: boolean;
  disabled: boolean;
  values: Pick<DeviceSettingsUpdateRequest, LedBrightnessField>;
  onChange: (field: LedBrightnessField, value: number) => void;
  t: ReturnType<typeof useTranslation>['t'];
}) {
  return (
    <DeviceSettingsPanel
      title={t('settings.device.ledZoneTitle', '灯光亮度')}
      desc={supported
        ? t('settings.device.ledZoneDesc', '四个灯区分别设置最大亮度，互不作为全局上限。')
        : t('settings.device.ledZoneUnsupported', '当前固件未回读四区亮度，灯区上限暂不可写入。')}
    >
      <div className="ol-device-led-grid">
        <LedBrightnessControl
          label={t('settings.device.statusLedBrightnessLabel', '状态灯')}
          desc={t('settings.device.statusLedBrightnessDesc', 'PWR / BLE / REC / AI / OK / WARN')}
          value={values.statusLedBrightnessPercent}
          disabled={disabled || !supported}
          onChange={value => onChange('statusLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.keyLedBrightnessLabel', '按键灯')}
          desc={t('settings.device.keyLedBrightnessDesc', 'KEY1-KEY4')}
          value={values.keyLedBrightnessPercent}
          disabled={disabled || !supported}
          onChange={value => onChange('keyLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.knobLedBrightnessLabel', '旋钮灯')}
          desc={t('settings.device.knobLedBrightnessDesc', 'EC11 环灯')}
          value={values.knobLedBrightnessPercent}
          disabled={disabled || !supported}
          onChange={value => onChange('knobLedBrightnessPercent', value)}
        />
        <LedBrightnessControl
          label={t('settings.device.edgeLedBrightnessLabel', '板框灯')}
          desc={t('settings.device.edgeLedBrightnessDesc', '边框氛围灯')}
          value={values.edgeLedBrightnessPercent}
          disabled={disabled || !supported}
          onChange={value => onChange('edgeLedBrightnessPercent', value)}
        />
      </div>
    </DeviceSettingsPanel>
  );
}

function LedBrightnessControl({
  label,
  desc,
  value,
  disabled,
  onChange,
}: {
  label: string;
  desc: string;
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  return (
    <div className="ol-device-led-row">
      <div className="ol-device-led-meta">
        <div style={{ minWidth: 0 }}>
          <div className="ol-device-led-title">{label}</div>
          <div className="ol-device-led-desc">{desc}</div>
        </div>
      </div>
      <PercentSlider value={value} disabled={disabled} onChange={onChange} />
      <PercentNumber value={value} disabled={disabled} onChange={onChange} />
    </div>
  );
}

function PercentNumber({
  value,
  disabled,
  onChange,
}: {
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const safeValue = clampPercent(value);
  const [draft, setDraft] = useState(() => String(safeValue));
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) {
      setDraft(String(safeValue));
    }
  }, [editing, safeValue]);

  const commit = (raw: string, fallback: number) => {
    const trimmed = raw.trim();
    if (trimmed.length === 0) {
      setDraft(String(fallback));
      return;
    }
    const next = Number(trimmed);
    if (!Number.isFinite(next)) {
      setDraft(String(fallback));
      return;
    }
    const clamped = clampPercent(next);
    onChange(clamped);
    setDraft(String(clamped));
  };

  const updateDraft = (raw: string) => {
    setDraft(raw);
    const trimmed = raw.trim();
    if (trimmed.length === 0) {
      return;
    }
    const next = Number(trimmed);
    if (!Number.isFinite(next)) {
      return;
    }
    const clamped = clampPercent(next);
    if (clamped !== value) {
      onChange(clamped);
    }
  };

  return (
    <div className="ol-device-percent-number">
      <input
        type="number"
        min={0}
        max={100}
        step={1}
        value={draft}
        disabled={disabled}
        onFocus={() => {
          setEditing(true);
          setDraft(String(safeValue));
        }}
        onChange={event => {
          updateDraft(event.target.value);
        }}
        onKeyDown={event => {
          if (event.key === 'Enter') {
            event.currentTarget.blur();
          } else if (event.key === 'Escape') {
            setDraft(String(safeValue));
            setEditing(false);
            event.currentTarget.blur();
          }
        }}
        onBlur={event => {
          setEditing(false);
          commit(event.target.value, safeValue);
        }}
        style={{ ...inputStyle, width: 56, height: 26, flex: '0 0 56px', maxWidth: 56, padding: '0 6px', textAlign: 'right' }}
      />
      <span style={{ fontSize: 11.5, color: 'var(--ol-ink-4)' }}>%</span>
    </div>
  );
}

function PercentSlider({
  value,
  disabled,
  onChange,
}: {
  value: number;
  disabled: boolean;
  onChange: (value: number) => void;
}) {
  const safeValue = clampPercent(value);
  return (
    <div className="ol-device-percent-slider" style={{ display: 'flex', alignItems: 'center', width: '100%', minWidth: 0 }}>
      <input
        className="ol-device-percent-range"
        type="range"
        min={0}
        max={100}
        step={1}
        value={safeValue}
        disabled={disabled}
        onChange={event => onChange(clampPercent(Number(event.target.value)))}
        style={{ width: '100%', minWidth: 0, accentColor: 'var(--ol-blue)', '--ol-device-range-percent': `${safeValue}%` } as CSSProperties}
      />
    </div>
  );
}

function KnobActionsPanel({
  knobClickMapping,
  knobRotationAction,
  installedApps,
  installedAppsLoading,
  onKnobClickMappingChange,
  onKnobRotationActionChange,
}: {
  knobClickMapping: DeviceCustomKeyMapping;
  knobRotationAction: DeviceKnobRotationAction;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  onKnobClickMappingChange: (mapping: DeviceCustomKeyMapping, commit?: boolean) => Promise<void>;
  onKnobRotationActionChange: (value: string) => Promise<void>;
}) {
  const { t } = useTranslation();

  return (
    <>
      <div style={{ fontSize: 13, fontWeight: 600, marginTop: 18, paddingTop: 14, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        {t('settings.deviceKeys.knob.title')}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', marginTop: 4, marginBottom: 8 }}>
        {t('settings.deviceKeys.knob.desc')}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        <div
          className="ol-device-knob-row"
          style={{
            display: 'grid',
            gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
            columnGap: DEVICE_KEY_LABEL_GAP,
            alignItems: 'center',
          }}
        >
          <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.deviceKeys.knob.gestures.rotate')}
          </div>
          <SelectLite
            value={knobRotationAction}
            onChange={value => void onKnobRotationActionChange(value)}
            options={KNOB_ROTATION_ACTIONS.map(action => ({
              value: action,
              label: t(`settings.deviceKeys.knob.actions.${action}`),
            }))}
            style={{ ...inputStyle, flex: `0 1 ${KNOB_CONTROL_WIDTH}px`, width: KNOB_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.knob.rotationActionSelectAria')}
          />
        </div>
        <div
          className="ol-device-knob-row"
          style={{
            display: 'grid',
            gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
            columnGap: DEVICE_KEY_LABEL_GAP,
            alignItems: 'start',
          }}
        >
          <div style={{ height: 32, display: 'flex', alignItems: 'center', fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)' }}>
            {t('settings.deviceKeys.knob.gestures.shortPress')}
          </div>
          <DeviceKeyMappingControl
            mapping={knobClickMapping}
            installedApps={installedApps}
            installedAppsLoading={installedAppsLoading}
            actionWidthOverride={KNOB_CONTROL_WIDTH}
            onChange={onKnobClickMappingChange}
          />
        </div>
        {KNOB_FIXED_ACTION_ROWS.map(item => (
          <div
            key={item.gesture}
            className="ol-device-knob-row"
            style={{
              display: 'grid',
              gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
              columnGap: DEVICE_KEY_LABEL_GAP,
              alignItems: 'center',
              opacity: 0.66,
            }}
          >
            <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink-3)' }}>
              {t(`settings.deviceKeys.knob.gestures.${item.gesture}`)}
            </div>
            <div
              aria-disabled
              style={{
                minHeight: 32,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 8,
                width: KNOB_CONTROL_WIDTH,
                maxWidth: '100%',
                boxSizing: 'border-box',
                padding: '0 10px',
                borderRadius: 6,
                background: 'var(--ol-surface-2)',
                border: '0.5px solid var(--ol-line-strong)',
                color: 'var(--ol-ink-4)',
                fontSize: 12,
              }}
            >
              <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {t(`settings.deviceKeys.knob.actions.${item.action}`)}
              </span>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

function DeviceKeyGestureGroup({
  gesture,
  keys,
  installedApps,
  installedAppsLoading,
  autoOpenDeviceKeyActionMenu,
  onChange,
}: {
  gesture: (typeof DEVICE_GESTURES)[number];
  keys: DeviceCustomKeys;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  autoOpenDeviceKeyActionMenu: boolean;
  onChange: (id: DeviceCustomKeyId, mapping: DeviceCustomKeyMapping, commit?: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  return (
    <div style={{ borderTop: '0.5px solid var(--ol-line-soft)', paddingTop: 12 }}>
      <div style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--ol-ink)', marginBottom: 8 }}>
        {t(`settings.deviceKeys.gestures.${gesture.id}`)}
      </div>
      <div style={{ display: 'flex', flexDirection: 'column' }}>
        {DEVICE_KEYS.map(({ id }, index) => (
          <div
            key={`${gesture.id}-${id}`}
            className="ol-device-key-row"
            style={{
              display: 'grid',
              gridTemplateColumns: DEVICE_KEY_ROW_TEMPLATE,
              columnGap: DEVICE_KEY_LABEL_GAP,
              alignItems: 'start',
              minWidth: 0,
              padding: index === 0 ? '4px 0 8px' : '9px 0 8px',
              borderTop: index === 0 ? 'none' : '0.5px solid var(--ol-line-soft)',
            }}
          >
            <div
              className="ol-device-key-label-cell"
              style={{
                minWidth: 0,
                height: 32,
                display: 'flex',
                alignItems: 'center',
              }}
            >
              <div
                className="ol-device-key-label"
                style={{
                  color: 'var(--ol-ink)',
                  fontSize: 12.5,
                  fontWeight: 700,
                  lineHeight: 1,
                }}
              >
                {t('settings.deviceKeys.keyLabel', { key: id.toUpperCase() })}
              </div>
            </div>
            <DeviceKeyMappingControl
              mapping={keys[id]}
              installedApps={installedApps}
              installedAppsLoading={installedAppsLoading}
              autoOpen={autoOpenDeviceKeyActionMenu && gesture.id === 'singleClick' && id === 'key1'}
              onChange={(mapping,commit) => onChange(id, mapping,commit)}
            />
          </div>
        ))}
      </div>
    </div>
  );
}

function DeviceKeyMappingControl({
  mapping,
  installedApps,
  installedAppsLoading,
  autoOpen = false,
  actionWidthOverride,
  onChange,
}: {
  mapping: DeviceCustomKeyMapping;
  installedApps: InstalledApplication[];
  installedAppsLoading: boolean;
  autoOpen?: boolean;
  actionWidthOverride?: number;
  onChange: (mapping: DeviceCustomKeyMapping, commit?: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  const externalAppPath = mapping.externalAppPath ?? '';
  const matchingInstalledApp = useMemo(
    () => installedApps.find(app => app.path.toLocaleLowerCase() === externalAppPath.toLocaleLowerCase()),
    [externalAppPath, installedApps],
  );
  const externalAppOptions = useMemo(() => [
    {
      value: EXTERNAL_APP_MANUAL_VALUE,
      label: installedAppsLoading
        ? t('settings.deviceKeys.installedAppLoading')
        : t('settings.deviceKeys.installedAppManual'),
    },
    ...installedApps.map(app => ({
      value: app.path,
      label: app.name,
    })),
  ], [installedApps, installedAppsLoading, t]);
  const externalAppPickerValue = matchingInstalledApp?.path ?? EXTERNAL_APP_MANUAL_VALUE;
  const mainActionWidth = actionWidthOverride ?? DEVICE_KEY_MAIN_CONTROL_WIDTH;
  const shortcut = mapping.shortcut ?? fallbackShortcut();
  const controlBaseStyle = {
    ...inputStyle,
    height: 32,
    padding: '0 9px',
    borderRadius: 8,
    boxSizing: 'border-box' as const,
    maxWidth: '100%',
  };
  const detailInputStyle = {
    ...controlBaseStyle,
    height: 27,
    fontSize: 11.5,
    fontFamily: 'var(--ol-font-mono)',
    color: 'var(--ol-ink-3)',
    background: 'transparent',
    borderColor: 'var(--ol-line-soft)',
  };

  const updateAction = async (action: DeviceCustomKeyAction) => {
    await onChange({
      ...mapping,
      action,
      shortcut: action === 'sendShortcut' ? mapping.shortcut ?? fallbackShortcut() : mapping.shortcut,
      appPage: action === 'openApp' ? mapping.appPage ?? 'settingsDevice' : mapping.appPage,
      externalAppPath,
    });
  };

  return (
    <div
      className="ol-device-key-control-stack"
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        alignItems: 'flex-start',
        width: '100%',
        maxWidth: '100%',
        minWidth: 0,
      }}
    >
      <div
        className="ol-device-key-action-row"
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: DEVICE_KEY_CONTROL_GAP,
          alignItems: 'center',
          width: '100%',
          maxWidth: '100%',
          minWidth: 0,
        }}
      >
        <SelectLite
          value={mapping.action}
          onChange={value => void updateAction(value as DeviceCustomKeyAction)}
          options={DEVICE_KEY_ACTIONS.map(action => ({
            value: action,
            label: t(`settings.deviceKeys.actions.${action}`),
          }))}
          defaultOpen={autoOpen}
          style={{ ...controlBaseStyle, flex: `0 1 ${mainActionWidth}px`, width: mainActionWidth, maxWidth: '100%', minWidth: 0 }}
          ariaLabel={t('settings.deviceKeys.actionSelectAria')}
        />
        {mapping.action === 'openApp' && (
          <SelectLite
            value={mapping.appPage ?? 'settingsDevice'}
            onChange={value => void onChange({ ...mapping, appPage: value as DeviceCustomKeyAppPage })}
            options={DEVICE_KEY_APP_PAGES.map(page => ({
              value: page,
              label: t(`settings.deviceKeys.appPages.${page}`),
            }))}
            style={{ ...controlBaseStyle, flex: `0 1 ${DEVICE_KEY_SECONDARY_CONTROL_WIDTH}px`, width: DEVICE_KEY_SECONDARY_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.appPageSelectAria')}
          />
        )}
        {mapping.action === 'openExternalApp' && (
          <SelectLite
            value={externalAppPickerValue}
            onChange={value => {
              if (value === EXTERNAL_APP_MANUAL_VALUE) return;
              void onChange({ ...mapping, externalAppPath: value });
            }}
            options={externalAppOptions}
            style={{ ...controlBaseStyle, flex: `0 1 ${DEVICE_KEY_SECONDARY_CONTROL_WIDTH}px`, width: DEVICE_KEY_SECONDARY_CONTROL_WIDTH, maxWidth: '100%', minWidth: 0 }}
            ariaLabel={t('settings.deviceKeys.installedAppSelectAria')}
          />
        )}
      </div>
      {mapping.action === 'openExternalApp' && (
        <input
          value={externalAppPath}
          onChange={event => {
            const externalAppPath = event.target.value;
            void onChange({ ...mapping, externalAppPath },false);
          }}
          placeholder={
            installedApps.length === 0 && !installedAppsLoading
              ? t('settings.deviceKeys.installedAppEmpty')
              : t('settings.deviceKeys.externalAppPlaceholder')
          }
          title={externalAppPath}
          style={{ ...detailInputStyle, width: '100%', maxWidth: DEVICE_KEY_DETAIL_WIDTH, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}
        />
      )}
      {mapping.action === 'pasteTemplate' && (
        <input
          value={mapping.pasteTemplate}
          onChange={event => {
            const pasteTemplate = event.target.value;
            void onChange({ ...mapping, pasteTemplate },false);
          }}
          placeholder={t('settings.deviceKeys.templatePlaceholder')}
          style={{ ...detailInputStyle, width: '100%', maxWidth: DEVICE_KEY_DETAIL_WIDTH, minWidth: 0 }}
        />
      )}
      {mapping.action === 'sendShortcut' && (
        <div style={{ width: 240, maxWidth: '100%', minWidth: 0 }}>
          <ShortcutRecorder
            value={shortcut}
            alignRecordButton
            onSave={async shortcut => {
              if (isDeviceFallbackReservedPrimary(shortcut.primary)) {
                throw new Error('reserved device fallback hotkey');
              }
              await onChange({ ...mapping, shortcut });
            }}
          />
        </div>
      )}
    </div>
  );
}

function snapshotToForm(snapshot: DeviceSettingsSnapshot): DeviceSettingsUpdateRequest {
  return {
    statusLedBrightnessPercent: snapshot.statusLedBrightnessPercent,
    keyLedBrightnessPercent: snapshot.keyLedBrightnessPercent,
    knobLedBrightnessPercent: snapshot.knobLedBrightnessPercent,
    edgeLedBrightnessPercent: snapshot.edgeLedBrightnessPercent,
    pluggedLowPowerIdleMinutes: snapshot.pluggedLowPowerEnabled ? snapshot.pluggedLowPowerIdleMinutes : 0,
    batteryLowPowerIdleMinutes: snapshot.batteryLowPowerIdleMinutes,
    pluggedLowPowerEnabled: snapshot.pluggedLowPowerEnabled,
    voiceAutoStartEnabled: snapshot.voiceAutoStartEnabled,
    voiceAutoStopEnabled: snapshot.voiceAutoStopEnabled,
    pluggedAutoShutdownMinutes: 0,
    batteryAutoShutdownMinutes: minutesFromMsForDeviceForm(snapshot.batteryAutoShutdownMs),
    bleName: snapshot.bleName,
  };
}

function validateDeviceSettingsForm(
  form: DeviceSettingsUpdateRequest,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (
    !Number.isFinite(form.statusLedBrightnessPercent) || form.statusLedBrightnessPercent < 0 || form.statusLedBrightnessPercent > 100 ||
    !Number.isFinite(form.keyLedBrightnessPercent) || form.keyLedBrightnessPercent < 0 || form.keyLedBrightnessPercent > 100 ||
    !Number.isFinite(form.knobLedBrightnessPercent) || form.knobLedBrightnessPercent < 0 || form.knobLedBrightnessPercent > 100 ||
    !Number.isFinite(form.edgeLedBrightnessPercent) || form.edgeLedBrightnessPercent < 0 || form.edgeLedBrightnessPercent > 100
  ) {
    return t('settings.device.errorZoneBrightness', '灯光亮度必须在 0-100 之间。');
  }
  if (!Number.isFinite(form.pluggedLowPowerIdleMinutes) || form.pluggedLowPowerIdleMinutes < 0 || form.pluggedLowPowerIdleMinutes > 1440) {
    return t('settings.device.errorLowPowerIdle', '低功耗等待时间必须在 0-1440 分钟之间。');
  }
  if (!Number.isFinite(form.batteryLowPowerIdleMinutes) || form.batteryLowPowerIdleMinutes < 0 || form.batteryLowPowerIdleMinutes > 1440) {
    return t('settings.device.errorLowPowerIdle', '低功耗等待时间必须在 0-1440 分钟之间。');
  }
  if (!Number.isFinite(form.batteryAutoShutdownMinutes) || form.batteryAutoShutdownMinutes < 0 || form.batteryAutoShutdownMinutes > 1440) {
    return t('settings.device.errorAutoShutdown', '自动关机时间必须在 0-1440 分钟之间。');
  }
  if (!isValidBleName(form.bleName)) {
    return t('settings.device.errorBleName', '蓝牙名称必须是 1-29 个可打印 ASCII 字符，不能包含空格、引号、分号、等号或反斜杠。');
  }
  return '';
}

function formatDeviceSnapshotDetail(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!snapshot?.detail) return '';
  if (snapshot.source === 'lastKnown') {
    return t('settings.device.detailLastKnown', '已写入设备；本次读取回执暂时不可用，界面显示刚刚写入的值。');
  }
  if (snapshot.source === 'defaults' && snapshot.connected) {
    return t('settings.device.detailDefaults', '设备已连接，但暂时读不到配置，当前显示默认值。');
  }
  if (snapshot.source === 'unavailable') {
    return t('settings.device.detailUnavailable', '设备未连接；连接后可以读取和写入配置。');
  }
  return '';
}

function isValidBleName(value: string): boolean {
  if (value.length < 1 || value.length > 29) return false;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x20 || code > 0x7e) return false;
    if (value[index] === '"' || value[index] === '\'' || value[index] === ';' || value[index] === '=' || value[index] === '\\') return false;
  }
  return true;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, Math.trunc(value)));
}

function clampMinuteValue(value: number, min: 0 | 1): number {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(1440, Math.trunc(value)));
}

function minutesFromMsForDeviceForm(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0;
  return Math.max(1, Math.min(1440, Math.floor(value / 60000)));
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, timeoutMessage: string): Promise<T> {
  let timer: number | undefined;
  const timeout = new Promise<T>((_, reject) => {
    timer = window.setTimeout(() => {
      reject(new Error(timeoutMessage));
    }, timeoutMs);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) {
      window.clearTimeout(timer);
    }
  });
}

function formatDeviceSnapshotFooter(
  snapshot: DeviceSettingsSnapshot | null,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (!snapshot) {
    return '';
  }
  if (!snapshot.writeSupported) {
    return t('settings.device.readOnlyHint', '当前设备状态只读；连接到可写固件后可以写入。');
  }
  return '';
}
