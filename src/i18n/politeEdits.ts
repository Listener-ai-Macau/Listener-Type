const en = {
  title: 'Request wording', request: 'Suggest more courteous requests', pending: 'Finding wording suggestions…',
  cost: 'Sends the current result to your configured text service when clicked; normal charges may apply. Text stays unchanged until you choose edits.',
  review: 'Service suggestions · {{time}} s. Check meaning and names, then choose the edits you want.',
  none: 'No usable request edits returned · {{time}} s. Your text is unchanged.',
  skipped: 'Suggestions that changed protected numbers, dates or firm requirements were skipped.',
  apply: 'Use selected edits', keep: 'Keep my wording', stale: 'Your text changed. These earlier suggestions cannot be applied; request new ones if wanted.',
  applied: 'Chosen edits applied locally. Review, copy or save when ready.',
  failed: 'Could not get usable suggestions. Your text is kept. Check the text service or try again.',
  native: 'Install Type to request suggestions with your configured text service. Your text is kept.',
};
export const politeEditsTranslations = {
  en,
  'zh-CN': { title: '请求措辞', request: '建议更礼貌的请求措辞', pending: '正在获取措辞建议…', cost: '点击后会把当前结果发给已配置的文本服务，可能正常计费。选中并接受前不会改动正文。', review: '服务建议 · {{time}} 秒。核对含义和名字，再选择需要的修改。', none: '未返回可用的请求修改 · {{time}} 秒，正文保持原样。', skipped: '改变受保护数字或明确强制要求的建议已跳过。', apply: '采用选中的修改', keep: '保留我的措辞', stale: '正文已改变，不能套用之前的建议；需要时可重新获取。', applied: '已在本地采用选中的修改，可核对、复制或保存。', failed: '暂时无法取得可用建议，正文已保留。可检查文本服务或重试。', native: '安装 Type 后可使用已配置的文本服务获取建议，正文已保留。' },
  'zh-TW': { title: '請求措辭', request: '建議更禮貌的請求措辭', pending: '正在取得措辭建議…', cost: '點擊後會把目前結果傳給已設定的文字服務，可能正常計費。選取並接受前不會改動正文。', review: '服務建議 · {{time}} 秒。核對含義和名字，再選擇需要的修改。', none: '未傳回可用的請求修改 · {{time}} 秒，正文保持原樣。', skipped: '改變受保護數字或明確強制要求的建議已跳過。', apply: '採用選取的修改', keep: '保留我的措辭', stale: '正文已改變，不能套用之前的建議；需要時可重新取得。', applied: '已在本機採用選取的修改，可核對、複製或儲存。', failed: '暫時無法取得可用建議，正文已保留。可檢查文字服務或重試。', native: '安裝 Type 後可用已設定的文字服務取得建議，正文已保留。' },
  ja: { title: '依頼の表現', request: 'より丁寧な依頼表現を提案', pending: '表現の候補を取得中…', cost: '押すと現在の結果を設定済みのテキストサービスに送信し、通常の料金が発生する場合があります。候補を選んで採用するまで本文は変わりません。', review: 'サービスの候補 · {{time}} 秒。意味と名前を確認し、必要な変更を選んでください。', none: '適用できる依頼の変更はありません · {{time}} 秒。本文はそのままです。', skipped: '保護対象の数字や明確な義務を変える候補は除外しました。', apply: '選んだ変更を採用', keep: '元の表現を維持', stale: '本文が変わったため以前の候補は適用できません。必要なら再取得してください。', applied: '選んだ変更をこの画面に適用しました。確認してコピーまたは保存できます。', failed: '利用できる候補を取得できませんでした。本文は残っています。サービス設定を確認するか再試行できます。', native: 'Typeをインストールすると設定済みサービスで候補を取得できます。本文は残っています。' },
  ko: { title: '요청 표현', request: '더 정중한 요청 표현 제안', pending: '표현 제안을 가져오는 중…', cost: '누르면 현재 결과를 설정한 텍스트 서비스로 전송하며 일반 요금이 발생할 수 있습니다. 변경을 선택해 적용하기 전까지 본문은 바뀌지 않습니다.', review: '서비스 제안 · {{time}}초. 의미와 이름을 확인한 후 원하는 변경을 선택하세요.', none: '사용 가능한 요청 수정이 없습니다 · {{time}}초. 본문은 그대로입니다.', skipped: '보호된 숫자나 명확한 필수 요구를 바꾸는 제안은 제외했습니다.', apply: '선택한 변경 적용', keep: '내 표현 유지', stale: '본문이 변경되어 이전 제안을 적용할 수 없습니다. 필요하면 새로 요청하세요.', applied: '선택한 변경을 로컬에 적용했습니다. 확인 후 복사하거나 저장하세요.', failed: '사용 가능한 제안을 가져오지 못했습니다. 본문은 유지됩니다. 서비스 설정을 확인하거나 다시 시도하세요.', native: 'Type을 설치하면 설정한 텍스트 서비스로 제안을 요청할 수 있습니다. 본문은 유지됩니다.' },
} satisfies Record<string, typeof en>;
