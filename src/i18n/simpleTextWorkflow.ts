const en = {
  workspace: 'Work with text', workspaceHint: 'Edit, draft or translate text, then copy or save it.',
  savedHint: 'Find the input drafts and finished text you saved.',
  toolsHint: 'Work with text, find saved writing, or set up dictation tools.',
  setup: 'Dictation setup', dictationStyles: 'Dictation styles', translationSettings: 'Translation settings', deviceKeys: 'Device keys and phrases',
  task: 'What would you like to do?', refine: 'Edit existing text', draft: 'Draft from instructions', translate: 'Translate text',
  draftFormat: 'Writing format', messageBody: 'Message body', email: 'Email subject and body',
  styleLabel: 'Writing style',
};
export const simpleTextWorkflowMessages = {
  en,
  'zh-CN': {
    workspace: '文字处理', workspaceHint: '整理、起草或翻译文字，完成后复制或保存。',
    savedHint: '找回你保存的输入草稿和处理后的文字。', toolsHint: '处理文字、找回已保存内容，或调整听写工具。',
    setup: '听写辅助设置', dictationStyles: '听写风格', translationSettings: '翻译设置', deviceKeys: '设备按键与短语',
    task: '想做什么', refine: '整理已有文字', draft: '按要求起草', translate: '翻译文字',
    draftFormat: '写作类型', messageBody: '消息正文', email: '邮件主题和正文',
    styleLabel: '文字风格',
  },
  'zh-TW': {
    workspace: '文字處理', workspaceHint: '整理、起草或翻譯文字，完成後複製或儲存。',
    savedHint: '找回你儲存的輸入草稿與處理後的文字。', toolsHint: '處理文字、找回已儲存內容，或調整聽寫工具。',
    setup: '聽寫輔助設定', dictationStyles: '聽寫風格', translationSettings: '翻譯設定', deviceKeys: '裝置按鍵與短語',
    task: '想做什麼', refine: '整理現有文字', draft: '依要求起草', translate: '翻譯文字',
    draftFormat: '寫作類型', messageBody: '訊息正文', email: '郵件主旨與正文',
    styleLabel: '文字風格',
  },
  ja: {
    workspace: '文章を編集・作成', workspaceHint: '文章の編集、下書き、翻訳を行い、コピーや保存ができます。',
    savedHint: '保存した入力文や完成した文章を開きます。', toolsHint: '文章を扱う、保存した文章を開く、音声入力を設定する。',
    setup: '音声入力の設定', dictationStyles: '音声入力のスタイル', translationSettings: '翻訳の設定', deviceKeys: 'デバイスのキーと定型文',
    task: '何をしますか', refine: '既存の文章を整える', draft: '指示から下書きを作る', translate: '文章を翻訳する',
    draftFormat: '文章の種類', messageBody: 'メッセージ本文', email: 'メールの件名と本文',
    styleLabel: '文章スタイル',
  },
  ko: {
    workspace: '텍스트 편집·작성', workspaceHint: '텍스트를 다듬거나 작성·번역한 뒤 복사하거나 저장하세요.',
    savedHint: '저장한 입력 초안과 처리한 글을 다시 엽니다.', toolsHint: '텍스트 작업, 저장한 글 열기 또는 받아쓰기 도구 설정.',
    setup: '받아쓰기 설정', dictationStyles: '받아쓰기 문체', translationSettings: '번역 설정', deviceKeys: '기기 키와 문구',
    task: '무엇을 할까요', refine: '기존 글 다듬기', draft: '요청으로 초안 작성', translate: '텍스트 번역',
    draftFormat: '글 종류', messageBody: '메시지 본문', email: '이메일 제목과 본문',
    styleLabel: '문체',
  },
} satisfies Record<string, typeof en>;
