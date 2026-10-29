const en = {
  name: 'Draft from instructions',
  description: 'Turn your writing requirements into a message body. Keep the facts and review before sending.',
  add: 'Draft from my instructions', choose: 'Use drafting style',
  hint: 'Use a style to refine an existing message. For instructions such as “ask for confirmation by Friday”, draft the message here. This prepares a separate style without changing dictation; only submitting uses your text service.',
  failed: 'Could not prepare the drafting style. Your text is kept; the current style stays selected.',
};
const bodyTranslations = {
  en,
  'zh-CN': { name: '按要求起草正文', description: '把写作要求整理成消息正文，保留事实，发送前请核对。', add: '按我的要求起草正文', choose: '使用起草风格', hint: '已有正文用风格润色；像“请对方周五确认”这样的写作要求，用这里起草成正文。会准备独立风格，不改变日常听写；明确提交后才使用文本服务。', failed: '未能准备起草风格，文字已保留，仍使用当前风格。' },
  'zh-TW': { name: '依要求起草正文', description: '將寫作要求整理成訊息正文，保留事實，傳送前請核對。', add: '依我的要求起草正文', choose: '使用起草風格', hint: '已有正文用風格潤飾；像「請對方週五確認」這樣的寫作要求，用這裡起草成正文。會準備獨立風格，不改變日常聽寫；明確提交後才使用文字服務。', failed: '無法準備起草風格，文字已保留，仍使用目前風格。' },
  ja: { name: '指示から下書きを作成', description: '文章の要件から本文を作成します。事実を保ち、送信前に確認してください。', add: '指示から本文を作成', choose: '下書きスタイルを使う', hint: '既存の本文は文章スタイルで整えます。「金曜までに確認してもらう」のような作成指示は、ここで本文にします。音声入力の設定を変えずに別のスタイルを準備し、処理ボタンを押したときだけテキストサービスを使います。', failed: '下書きスタイルを準備できませんでした。文章と現在のスタイルは維持されています。' },
  ko: { name: '요청으로 초안 작성', description: '작성 요구사항을 메시지 본문으로 정리합니다. 사실을 유지하며 보내기 전에 확인하세요.', add: '내 요청으로 본문 작성', choose: '초안 스타일 사용', hint: '이미 쓴 본문은 스타일로 다듬으세요. “금요일까지 확인을 요청하는 메시지” 같은 작성 요구사항은 여기서 본문으로 만듭니다. 받아쓰기 설정을 바꾸지 않고 별도 스타일을 준비하며, 처리 버튼을 눌러야 텍스트 서비스를 사용합니다.', failed: '초안 스타일을 준비하지 못했습니다. 텍스트와 현재 스타일은 유지됩니다.' },
} satisfies Record<string, typeof en>;
export const messageDraftTranslations = {
  en: { ...bodyTranslations.en, emailName: 'Draft email with subject', emailDescription: 'Draft an email subject and body from your instructions. Check both before sending.', emailAdd: 'Draft an email with subject', emailChoose: 'Use email drafting style', emailHint: 'Choose this when you also want a suggested subject. Choosing prepares a separate style; only submitting uses your text service. Subject and body remain editable.', emailFailed: 'Could not prepare the email drafting style. Your text and current style are kept.' },
  'zh-CN': { ...bodyTranslations['zh-CN'], emailName: '起草邮件主题和正文', emailDescription: '根据要求起草邮件主题和正文，发送前请核对。', emailAdd: '起草邮件主题和正文', emailChoose: '使用邮件起草风格', emailHint: '需要邮件主题时选择此项。选择只准备独立风格，明确提交才使用文本服务；主题和正文都可编辑。', emailFailed: '未能准备邮件起草风格，文字与当前风格已保留。' },
  'zh-TW': { ...bodyTranslations['zh-TW'], emailName: '起草郵件主旨和正文', emailDescription: '根據要求起草郵件主旨和正文，傳送前請核對。', emailAdd: '起草郵件主旨和正文', emailChoose: '使用郵件起草風格', emailHint: '需要郵件主旨時選擇此項。選擇只準備獨立風格，明確提交才使用文字服務；主旨和正文都可編輯。', emailFailed: '無法準備郵件起草風格，文字與目前風格已保留。' },
  ja: { ...bodyTranslations.ja, emailName: '件名付きメールを作成', emailDescription: '指示からメールの件名と本文を作成します。送信前に確認してください。', emailAdd: '件名付きメールを作成', emailChoose: 'メール作成スタイルを使う', emailHint: '件名も必要な場合に選択します。選択時は別のスタイルを準備するだけで、送信時にテキストサービスを使います。件名と本文は編集できます。', emailFailed: 'メール作成スタイルを準備できませんでした。文章と現在のスタイルは保持されます。' },
  ko: { ...bodyTranslations.ko, emailName: '제목과 이메일 초안 작성', emailDescription: '요청에 따라 이메일 제목과 본문을 작성합니다. 보내기 전에 확인하세요.', emailAdd: '제목과 이메일 초안 작성', emailChoose: '이메일 작성 스타일 사용', emailHint: '제목도 필요할 때 선택하세요. 선택 시 별도 스타일만 준비하며 제출해야 텍스트 서비스를 사용합니다. 제목과 본문은 수정할 수 있습니다.', emailFailed: '이메일 작성 스타일을 준비하지 못했습니다. 글과 현재 스타일은 유지됩니다.' },
};
