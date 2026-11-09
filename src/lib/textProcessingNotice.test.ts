import assert from 'node:assert/strict';
import { textProcessingNotice } from './textProcessingNotice.ts';

const unavailable = { available: false, requiresTextService: false, textServiceConfigured: false };
assert.equal(textProcessingNotice(false, false, unavailable, true), 'textWorkspace.previewProcessingNotice');
assert.equal(textProcessingNotice(true, false, unavailable, true), 'textWorkspace.processingUnconfirmed');
assert.equal(textProcessingNotice(true, false, null, false), 'textWorkspace.processingUnconfirmed');
assert.equal(textProcessingNotice(true, false, { ...unavailable, available: true, requiresTextService: true }, false), 'textWorkspace.cloudNotice');
assert.equal(textProcessingNotice(true, false, { ...unavailable, available: true }, true), 'textWorkspace.localWithShorterNotice');
assert.equal(textProcessingNotice(true, false, { ...unavailable, available: true }, false), 'textWorkspace.localNotice');
assert.equal(textProcessingNotice(true, true, null, false), 'offlineTranslation.localNotice');
assert.equal(textProcessingNotice(false, true, null, false), 'textWorkspace.previewProcessingNotice');
console.log('PASS unknown/unavailable/preview cannot promise local processing; native local/cloud routes stay explicit');
