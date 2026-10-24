const en = {
  editCopy: 'Edit a copy', copyEditorTitle: 'Edit a draft copy', copyEditorText: 'Draft copy text',
  copyEditorHint: 'Save this as a new draft. The original draft and its source stay unchanged.',
  saveCopy: 'Save as new draft', copySaved: 'New draft saved', backToDrafts: 'Back to saved drafts',
  discardCopyConfirm: 'This copy has unsaved changes. Discard them? The original draft stays saved.',
  discardCopy: 'Discard changes',
};
export const savedDraftCopyMessages = {
  en,
  'zh-CN': {
    editCopy: '编辑副本', copyEditorTitle: '编辑成稿副本', copyEditorText: '成稿副本正文',
    copyEditorHint: '修改后另存为一份新成稿，原成稿及随附原文保持不变。',
    saveCopy: '保存为新成稿', copySaved: '新成稿已保存', backToDrafts: '返回成稿列表',
    discardCopyConfirm: '副本有未保存的修改，确定放弃吗？原成稿仍然保留。', discardCopy: '放弃修改',
  },
  'zh-TW': {
    editCopy: '編輯副本', copyEditorTitle: '編輯成稿副本', copyEditorText: '成稿副本正文',
    copyEditorHint: '修改後另存為一份新成稿，原成稿及隨附原文維持不變。',
    saveCopy: '另存為新成稿', copySaved: '新成稿已儲存', backToDrafts: '返回成稿清單',
    discardCopyConfirm: '副本有尚未儲存的修改，確定放棄嗎？原成稿仍然保留。', discardCopy: '放棄修改',
  },
  ja: {
    editCopy: 'コピーを編集', copyEditorTitle: '下書きのコピーを編集', copyEditorText: 'コピーの本文',
    copyEditorHint: '新しい下書きとして保存します。元の下書きと原文は変更しません。',
    saveCopy: '新しい下書きとして保存', copySaved: '新しい下書きを保存しました', backToDrafts: '保存した下書きに戻る',
    discardCopyConfirm: 'コピーに未保存の変更があります。破棄しますか？元の下書きは残ります。', discardCopy: '変更を破棄',
  },
  ko: {
    editCopy: '사본 편집', copyEditorTitle: '초안 사본 편집', copyEditorText: '초안 사본 본문',
    copyEditorHint: '새 초안으로 저장합니다. 원래 초안과 원문은 변경되지 않습니다.',
    saveCopy: '새 초안으로 저장', copySaved: '새 초안 저장됨', backToDrafts: '저장한 초안으로 돌아가기',
    discardCopyConfirm: '사본에 저장하지 않은 변경이 있습니다. 버릴까요? 원래 초안은 유지됩니다.', discardCopy: '변경 버리기',
  },
} satisfies Record<string, typeof en>;
