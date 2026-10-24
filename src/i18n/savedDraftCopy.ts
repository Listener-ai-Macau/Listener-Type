const en = {
  keyPhraseHint: 'Choose a key and press type, then use this message in its draft layout. You can practice it here and save the layout. Your saved message stays unchanged; this does not write to a device.',
  editCopy: 'Edit a copy', copyEditorTitle: 'Edit a draft copy', copyEditorText: 'Draft copy text',
  copyEditorHint: 'Save this as a new draft. The original draft and its source stay unchanged.',
  saveCopy: 'Save as new draft', copySaved: 'New draft saved', backToDrafts: 'Back to saved drafts',
  discardCopyConfirm: 'This copy has unsaved changes. Discard them? The original draft stays saved.',
  discardCopy: 'Discard changes',
};
export const savedDraftCopyMessages = {
  en,
  'zh-CN': {
    keyPhraseHint: '选择按键和触发方式，再将消息用于布局草稿。可在这里练习并保存布局；已保存成稿保持不变，此操作不写入设备。',
    editCopy: '编辑副本', copyEditorTitle: '编辑成稿副本', copyEditorText: '成稿副本正文',
    copyEditorHint: '修改后另存为一份新成稿，原成稿及随附原文保持不变。',
    saveCopy: '保存为新成稿', copySaved: '新成稿已保存', backToDrafts: '返回成稿列表',
    discardCopyConfirm: '副本有未保存的修改，确定放弃吗？原成稿仍然保留。', discardCopy: '放弃修改',
  },
  'zh-TW': {
    keyPhraseHint: '選擇按鍵與觸發方式，再將訊息用於配置草稿。可在這裡練習並儲存配置；已儲存成稿維持不變，此操作不寫入裝置。',
    editCopy: '編輯副本', copyEditorTitle: '編輯成稿副本', copyEditorText: '成稿副本正文',
    copyEditorHint: '修改後另存為一份新成稿，原成稿及隨附原文維持不變。',
    saveCopy: '另存為新成稿', copySaved: '新成稿已儲存', backToDrafts: '返回成稿清單',
    discardCopyConfirm: '副本有尚未儲存的修改，確定放棄嗎？原成稿仍然保留。', discardCopy: '放棄修改',
  },
  ja: {
    keyPhraseHint: 'キーと押し方を選び、このメッセージをキー設定の下書きに使います。画面上で練習し、設定を保存できます。保存済みの本文は変更せず、デバイスには書き込みません。',
    editCopy: 'コピーを編集', copyEditorTitle: '下書きのコピーを編集', copyEditorText: 'コピーの本文',
    copyEditorHint: '新しい下書きとして保存します。元の下書きと原文は変更しません。',
    saveCopy: '新しい下書きとして保存', copySaved: '新しい下書きを保存しました', backToDrafts: '保存した下書きに戻る',
    discardCopyConfirm: 'コピーに未保存の変更があります。破棄しますか？元の下書きは残ります。', discardCopy: '変更を破棄',
  },
  ko: {
    keyPhraseHint: '키와 누르는 방식을 고른 뒤 이 메시지를 키 설정 초안에 사용하세요. 화면에서 연습하고 설정을 저장할 수 있습니다. 저장한 메시지는 변경되지 않으며 기기에 쓰지 않습니다.',
    editCopy: '사본 편집', copyEditorTitle: '초안 사본 편집', copyEditorText: '초안 사본 본문',
    copyEditorHint: '새 초안으로 저장합니다. 원래 초안과 원문은 변경되지 않습니다.',
    saveCopy: '새 초안으로 저장', copySaved: '새 초안 저장됨', backToDrafts: '저장한 초안으로 돌아가기',
    discardCopyConfirm: '사본에 저장하지 않은 변경이 있습니다. 버릴까요? 원래 초안은 유지됩니다.', discardCopy: '변경 버리기',
  },
} satisfies Record<string, typeof en>;
