import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Btn, Card, Pill } from '../_atoms';

export function CompanionSection() {
  return <CompanionBringupCard />;
}

function CompanionBringupCard() {
  const { t } = useTranslation();
  const [copied, setCopied] = useState('');
  const commands = [
    {
      id: 'build',
      label: t('settings.companion.buildCommand', '编译 bring-up'),
      value: 'pwsh -NoProfile -File .\\tools\\build_nucleo_bringup.ps1',
    },
    {
      id: 'flash',
      label: t('settings.companion.flashCommand', '刷入 NUCLEO'),
      value: 'pwsh -NoProfile -File .\\tools\\flash_nucleo_bringup.ps1',
    },
    {
      id: 'capture',
      label: t('settings.companion.captureCommand', '抓串口日志'),
      value: 'pwsh -NoProfile -File .\\tools\\capture_nucleo_serial.ps1 -Port COM13 -Seconds 30',
    },
  ];
  const copy = async (id: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(id);
      window.setTimeout(() => setCopied(current => (current === id ? '' : current)), 1400);
    } catch (error) {
      console.warn('[companion] copy command failed', error);
    }
  };

  return (
    <Card style={{ padding: 18 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12, marginBottom: 12 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
            <div style={{ fontSize: 15, fontWeight: 700 }}>
              {t('settings.companion.title', 'Companion Pendant bring-up')}
            </div>
            <Pill tone="blue" size="sm">{t('settings.companion.badge', 'NUCLEO-WB55RG')}</Pill>
          </div>
          <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5, marginTop: 5 }}>
            {t('settings.companion.desc', '先用开发板验证麦克风、外部 flash、OTA readiness 和按钮输入；正式产品工程从 Companion_Pendant.ioc 生成。')}
          </div>
        </div>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(168px, 1fr))', gap: 8 }}>
        <CompanionFact
          label={t('settings.companion.sw1Label', 'SW1 / PC4')}
          value={t('settings.companion.sw1Value', '开始 / 停止录音')}
        />
        <CompanionFact
          label={t('settings.companion.sw2Label', 'SW2 / PD0')}
          value={t('settings.companion.sw2Value', '取消并回 idle')}
        />
        <CompanionFact
          label={t('settings.companion.sw3Label', 'SW3 / PD1')}
          value={t('settings.companion.sw3Value', 'flash + OTA + diag')}
        />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))', gap: 8, marginTop: 12 }}>
        <CompanionFact
          label={t('settings.companion.bringupProjectLabel', '当前硬件工程')}
          value="cubeide/Companion_Nucleo_WB55RG_Bringup"
          mono
        />
        <CompanionFact
          label={t('settings.companion.productIocLabel', '正式产品 .ioc')}
          value="Companion_Pendant.ioc"
          mono
        />
      </div>

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 12, paddingTop: 12, borderTop: '0.5px solid var(--ol-line-soft)' }}>
        {commands.map(command => (
          <Btn
            key={command.id}
            variant={copied === command.id ? 'blue' : 'ghost'}
            size="sm"
            icon={copied === command.id ? 'check' : 'copy'}
            onClick={() => void copy(command.id, command.value)}
          >
            {copied === command.id ? t('common.copied') : command.label}
          </Btn>
        ))}
      </div>
    </Card>
  );
}

function CompanionFact({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div
      style={{
        minWidth: 0,
        padding: '8px 10px',
        borderRadius: 7,
        border: '0.5px solid var(--ol-line-soft)',
        background: 'var(--ol-control-track)',
      }}
    >
      <div style={{ fontSize: 10.5, color: 'var(--ol-ink-4)', marginBottom: 3 }}>{label}</div>
      <div
        title={value}
        style={{
          fontSize: 11.5,
          color: 'var(--ol-ink)',
          fontFamily: mono ? 'var(--ol-font-mono)' : undefined,
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
      >
        {value}
      </div>
    </div>
  );
}
