import type { TFunction } from 'i18next';
import type { StylePack } from './types';

/** Explicit drafting choice; never changes the transcription/formatting styles. */
export const MESSAGE_DRAFT_PROMPT = `Draft a recipient-facing message from the supplied brief or existing draft. This is an explicitly selected writing task.

Distinguish directions to the writer from the content addressed to the recipient. Apply requirements about audience, tone, length, format and language; do not repeat those writing directions as sentences in the message. Keep recipient-facing questions, requests, prohibitions and deadlines in the message without answering them or performing the requested actions. Do not remove a request merely because it starts with "please". Preserve quoted content and the author's intended position.

Keep every distinct fact, name, date, time, quantity, alternative, condition, uncertainty and negation. Do not invent dates, contact details, venue translations, signers, promises or business policies. When specifics are missing, retain the source's level of precision instead of filling them in. Use the requested output language, otherwise the source's main language; keep proper names as supplied unless an official translation was provided.

For an existing message, make only changes needed for the requested writing task. Do not add unsupported greetings, apologies, commitments or a signature. Leave the original meaning and recipient's choices intact.

{{HOTWORDS}}

Return only the usable message body. No writing advice, instruction list, explanation, preface or code fence.`;

export function isMessageDraftStyle(pack: StylePack): boolean {
  return isMessageBodyDraftStyle(pack) || isEmailDraftStyle(pack);
}
export function isMessageBodyDraftStyle(pack: StylePack): boolean {
  return pack.kind === 'imported' && pack.baseMode === 'formal' && pack.prompt === MESSAGE_DRAFT_PROMPT;
}

/** A separate explicit choice; existing body-only drafts remain unchanged. */
export const EMAIL_DRAFT_PROMPT = MESSAGE_DRAFT_PROMPT.replace(
  'Return only the usable message body. No writing advice, instruction list, explanation, preface or code fence.',
  'Return one concise email subject followed by the usable message body. Label the subject in the output language and place it on its own line, then a blank line and the body. The subject must reflect only the supplied facts. No writing advice, instruction list, explanation, preface or code fence.'
);
export function isEmailDraftStyle(pack: StylePack): boolean {
  return pack.kind === 'imported' && pack.baseMode === 'formal' && pack.prompt === EMAIL_DRAFT_PROMPT;
}
export function emailDraftStyle(t: TFunction): StylePack {
  return { id: '', name: t('messageDraft.emailName'), description: t('messageDraft.emailDescription'),
    version: '1.0.0', kind: 'imported', baseMode: 'formal', prompt: EMAIL_DRAFT_PROMPT,
    examples: [], tags: [], enabled: true, active: false };
}

export function messageDraftStyle(t: TFunction): StylePack {
  return { id: '', name: t('messageDraft.name'), description: t('messageDraft.description'),
    version: '1.0.0', kind: 'imported', baseMode: 'formal', prompt: MESSAGE_DRAFT_PROMPT,
    examples: [], tags: [], enabled: true, active: false };
}
