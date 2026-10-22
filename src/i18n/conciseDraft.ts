const en = {
  name: 'Concise message',
  description: 'Shorten wordy phrasing while retaining the message’s details, conditions and language. Already concise text may stay unchanged. Review before using.',
  add: 'Add concise style', choose: 'Use concise style', adding: 'Adding style…',
  hint: 'Saves a style on this computer without activating it or sending text. Choose Try this text to process with your text service.',
  failed: 'Could not prepare the concise style. Your draft is kept; you can continue with the current style.',
  run: 'Make shorter', running: 'Shortening…',
  runHint: 'Make shorter uses your text service. Your original stays; a shorter version completed in this window is reused without another request. Review the result before using it.',
};
export const conciseDraftTranslations = {
  en,
  'zh-CN': { run: "缩短这段文字", running: "正在精简…", runHint: "使用文本服务精简，原稿保留；本窗口内已有的精简结果直接取回，不重复请求。采用前请核对。", name: '简洁成稿', description: '压缩啰嗦表达，保留信息、条件和原语言。已简洁的内容可能保持原样，采用前请核对。', add: '添加简洁风格', choose: '使用简洁风格', adding: '正在添加风格…', hint: '只在本机保存风格，不启用、不发送文字。明确点击“处理这段文字”后才使用文本服务。', failed: '未能准备简洁风格，草稿已保留，可继续使用当前风格。' },
  'zh-TW': { run: "縮短這段文字", running: "正在精簡…", runHint: "使用文字服務精簡，原稿保留；本視窗內已有的精簡結果直接取回，不重複請求。採用前請核對。", name: '簡潔成稿', description: '精簡冗長表達，保留資訊、條件和原語言。已簡潔的內容可能維持原樣，採用前請核對。', add: '新增簡潔風格', choose: '使用簡潔風格', adding: '正在新增風格…', hint: '只在本機儲存風格，不啟用、不傳送文字。明確點擊處理文字後才使用文字服務。', failed: '無法準備簡潔風格，草稿已保留，可繼續使用目前風格。' },
  ja: { run: "短くする", running: "短くしています…", runHint: "テキストサービスで短くします。原文を残し、このウィンドウ内の完成済みの短い結果は再リクエストせず表示します。使用前に確認してください。", name: '簡潔なメッセージ', description: '冗長な表現を短くし、詳細・条件・元の言語を保ちます。すでに簡潔な文章は変わらない場合があります。使用前に確認してください。', add: '簡潔なスタイルを追加', choose: '簡潔なスタイルを使う', adding: 'スタイルを追加中…', hint: 'この端末にスタイルを保存するだけで、有効化やテキスト送信はしません。処理ボタンを押すとテキストサービスを使います。', failed: '簡潔なスタイルを準備できませんでした。下書きは残っています。現在のスタイルを使えます。' },
  ko: { run: "더 짧게", running: "줄이는 중…", runHint: "텍스트 서비스를 사용해 줄입니다. 원문은 유지하며 이 창에서 완료된 결과는 추가 요청 없이 다시 표시합니다. 사용 전에 확인하세요.", name: '간결한 메시지', description: '장황한 표현을 줄이고 세부 정보, 조건, 원래 언어를 유지합니다. 이미 간결하면 바뀌지 않을 수 있습니다. 사용 전에 확인하세요.', add: '간결한 스타일 추가', choose: '간결한 스타일 사용', adding: '스타일 추가 중…', hint: '이 컴퓨터에 스타일만 저장하며 활성화하거나 텍스트를 전송하지 않습니다. 처리 버튼을 누르면 텍스트 서비스를 사용합니다.', failed: '간결한 스타일을 준비하지 못했습니다. 초안은 유지되며 현재 스타일을 계속 사용할 수 있습니다.' },
} satisfies Record<string, typeof en>;
