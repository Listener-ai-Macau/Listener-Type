const en = {
  name: 'Draft from instructions',
  description: 'Turn your writing requirements into a message body. Keep the facts and review before sending.',
  add: 'Draft from my instructions', choose: 'Use drafting style',
  hint: 'For a writing brief rather than an existing message. This prepares a separate style without changing dictation; submit when ready to use your text service.',
  failed: 'Could not prepare the drafting style. Your text is kept; the current style stays selected.',
};
export const messageDraftTranslations = {
  en,
  'zh-CN': { name: '按要求起草正文', description: '把写作要求整理成消息正文，保留事实，发送前请核对。', add: '按我的要求起草正文', choose: '使用起草风格', hint: '适合口述写作要求。会准备独立风格，不改变日常听写；明确提交后才使用文本服务。', failed: '未能准备起草风格，文字已保留，仍使用当前风格。' },
  'zh-TW': { name: '依要求起草正文', description: '將寫作要求整理成訊息正文，保留事實，傳送前請核對。', add: '依我的要求起草正文', choose: '使用起草風格', hint: '適合口述寫作要求。會準備獨立風格，不改變日常聽寫；明確提交後才使用文字服務。', failed: '無法準備起草風格，文字已保留，仍使用目前風格。' },
  ja: { name: '指示から下書きを作成', description: '文章の要件から本文を作成します。事実を保ち、送信前に確認してください。', add: '指示から本文を作成', choose: '下書きスタイルを使う', hint: '既存の文章ではなく、文章作成の要件に使います。音声入力の設定を変えずに別のスタイルを準備します。処理ボタンを押すとテキストサービスを使います。', failed: '下書きスタイルを準備できませんでした。文章と現在のスタイルは維持されています。' },
  ko: { name: '요청으로 초안 작성', description: '작성 요구사항을 메시지 본문으로 정리합니다. 사실을 유지하며 보내기 전에 확인하세요.', add: '내 요청으로 본문 작성', choose: '초안 스타일 사용', hint: '기존 메시지보다 작성 요구사항에 사용하세요. 받아쓰기 설정을 바꾸지 않고 별도 스타일을 준비합니다. 처리 버튼을 눌러야 텍스트 서비스를 사용합니다.', failed: '초안 스타일을 준비하지 못했습니다. 텍스트와 현재 스타일은 유지됩니다.' },
} satisfies Record<string, typeof en>;
