# Streaming dictation repair ledger — 2026-09-24

This is the current owner-visible defect list. The 1.0.5 executable
(`3E1EC6C2…`) supplied the initial observations below. The first 1.0.6 MSI
(`6C671872…`) supplied the 11:46–11:56 failures. A second 1.0.6 test MSI
(`5B447C6C…`) supplied the 12:12 continuous-speech observation. A third
1.0.6 test MSI was installed on 2026-09-24 at 12:22 China time; the running
executable matched its freshly built release payload (`76D2FBF6…`, product
version 1.0.6). A fourth 1.0.6 test MSI was installed at 12:53; its running
executable matches the new release payload (`34DB2BF9…`). Source and focused
unit checks passed; the fourth build still needs fresh device sessions before
release.

| Area | Reported behavior and live evidence | Root boundary and acceptance |
| --- | --- | --- |
| Wake | Wake sometimes feels slow; one false wake was reported. In `c61bda72`, the phrase passed at 11:26:15.639 despite an enrolled voiceprint non-match; first preview followed 661 ms later. | Preserve phrase-triggered opening and reject unrelated candidates. Measure wake phrase end → capsule visible → first provider text separately. |
| Preview | Preview can lag speech and only its first clause may stream. Frontend previously held recording bars for 900 ms after text arrived and animated short provider appends character by character. | Show each confirmed short provider update immediately, without a separate per-character timer or whole-sentence wait. Verify provider callback → frontend event → rendered text on the installed build. |
| Mid-session streaming decay | Owner reports that an initially responsive session gradually stops keeping up, especially after a pause. The 11:55 `67577453` first build session pasted 38 characters, then the provider rewrote the opening and the live prefix no longer aligned; finalization skipped the remaining 56 characters. | Trace one session from wake through every provider revision, owner ledger, visual preview, stable-prefix gate, insertion and finalization. Keep the capsule reversible; resume insertion only at an unambiguous boundary after a rewrite, then confirm the later clause survives a pause and STOP on the installed build. |
| Pause and continuation | After a pause, text may stop growing or lose the resumed clause. `c61bda72` had two positive owner body windows but an unverified wake bank; its second speaker-1 owner row was removed, yielding 51 final chars after 115 preview chars. `3c2e0545` also had an unaligned delivered prefix and dropped its final tail. | Evaluate owner identity per audio interval, permit a later same-speaker row when the current body has positive owner evidence, and exclude independently verified foreign rows. A pause followed by more owner speech must keep appending through final delivery. |
| Repeated or revised insertion | User observed duplicate text and lost punctuation. `3c2e0545` logged repeated 1–4 character pastes and `delivered_prefix_revised_unaligned`; `c61bda72` pasted 109 chars before final arbitration chose 51. | Irreversible committed text needs a stable owner boundary. Final reconciliation must preserve a valid appended suffix without pasting the whole transcript again; punctuation must survive the boundary. Confirm actual target text, not only `PasteSent` events. |
| Slow completion | `137bc74b` lost its provider stream and replayed an already available full recording at STOP, taking about 4.1 s. `d409d5c8` replay diarization then cut a continuation visible in the original stream. | Use the original session ledger when it covers the last owner speech; replay only for uncovered audio or unresolved foreign speech. Measure STOP → final text → capsule idle. |
| Interference | Other voices sometimes enter preview or inserted text. In `c61bda72`, provider speaker 0 interleaved with the owner's speaker 1; local score was often Uncertain, and preview/terminal filtering disagreed. | Keep cloud diarization and enrolled/local speaker evidence as separate signals. Require corroboration before irreversible owner text commit; exclude known foreign rows, while preserving later owner speech after a brief intrusion. |
| Capsule transition | Recording, transcribing, done, and idle changes feel slow. | Remove presentation-only waits and verify state timestamps on a newly installed build. |

## Fresh 1.0.6 field findings before release

- `3e8ab1c5` (11:46): the provider had no text for its first 2.3 s, then
  revised the opening. A single stable character was pasted at 11:46:28;
  the later prefix no longer aligned, so final delivery skipped the tail.
  The capsule itself received each provider revision within milliseconds.
- `6c62818a` (11:50): a provisional `Her.` arrived first; the provider then
  replaced it with the wake phrase and owner sentence. The old streaming
  merge retained `Her.`. At final, an owner-preview length ceiling replaced
  a corrected provider candidate with that stale ledger, dropping final
  punctuation. The final frame also contained a later English tail, so the
  ceiling must still enforce an owner boundary while preserving corrected
  words and punctuation.
- `60250cb0` and `a1dab343` (11:53): stable-prefix delivery continued in
  1–5 character pastes. This feels like streaming stops working mid-sentence
  because the capsule and inserted text advance on different boundaries.
  Keep provisional letters in the capsule and deliver useful stable chunks.
- `67577453` (11:55–11:56): the first 38 characters were inserted in many
  tiny pastes. The provider then rewrote the opening, so the early ledger no
  longer matched the final 94-character owner result. The final path skipped
  the remainder. The repaired recovery looks for a unique long seam near the
  actual paste boundary, even when the opening changed, and appends only text
  after that seam.

The first repair pass after these sessions raises the irreversible paste chunk
floor, permits a unique long seam to recover a continuation at finalization,
and changes the stop ceiling to trim the corrected final at an aligned
owner-content boundary. These changes are in the second test MSI, but still
need a pause-and-resume device session for acceptance. The 11:46–11:53
sessions are evidence of failure, not release acceptance.

## Second 1.0.6 test build: live evidence

- `edf171aa` (12:12): a continuous owner utterance did keep streaming in the
  capsule from 2 to 110 characters. The first visible preview was 443 ms after
  wake acceptance. Stable-prefix pastes advanced from 7 to at least 102
  characters while speech continued. STOP to done was 132 ms and the final
  candidate contained 105 body characters. The paste route reports
  `PasteSent`, not target-confirmed text; this one continuous session does not
  prove that a pause followed by resumed owner speech works.
- In the same session, local owner scores were roughly 0.4–0.5 during the
  growing text, then fell to 0.08–0.21 in the last five seconds while VAD still
  saw speech energy. The provider's only speaker row ended at 17.761 s of
  22.719 s captured audio, and the endpoint fired as `ConfirmedOther` after
  three seconds. Determine whether this is actual background speech or a false
  owner rejection before changing the endpoint; otherwise a continuation can
  be cut or another voice can be inserted.
- Source follow-up: the live stable-prefix path still refused a continuation
  when the provider corrected the opening from character one, although final
  reconciliation already accepted a unique eight-character seam near the
  delivered boundary. The live path now applies that same narrow seam rule;
  a repeated seam is rejected. The regression test passed and this change is
  in the third installed test MSI; real pause-and-resume acceptance is pending.

The product priority remains `repair-order-a-g.md`: do not cut the owner's
words to make interference filtering appear stronger. Every result above needs
fresh installed-binary and device evidence; source tests alone are insufficient.

## Third 1.0.6 test build: new failure at 12:34

- `a22b7aac` recorded 54.76 s of PCM on the installed `76D2FBF6…` executable.
  The capsule showed only the first 3–5 characters at 2.5–3.0 s, then stayed
  unchanged for about 47 s. The primary provider accepted 700,800 PCM bytes
  (about 21.9 s) and last reported 17.3 s of audio; the retained capture held
  1,752,320 bytes (54.76 s). The server first revised its partial from nine
  characters to empty and had no final text. A full-audio replay at STOP also
  returned empty; final delivery used the stale five-character preview.
- The local speaker verifier was mostly `Uncertain` at about 0.05–0.35, with
  intermittent `NonTarget` and a few late `Target` windows, while local VAD
  saw sustained speech energy. This may be different or distant speech; the
  recording alone cannot label its owner. The clear transport boundary is that
  captured audio outlived the primary stream by more than 30 s. The last
  provider response arrived at 12:34:28, then the 4.5 s no-response watchdog
  would have aborted this previously responsive stream around 12:34:33; its
  21.9 s queue watermark matches that timing. That is a strong inference,
  although the rotated raw log no longer contains the abort line. The watchdog
  now aborts only a stream that has never responded. After a response it logs
  a result gap while keeping the socket open, allowing later speech to resume.
  Explicit close, error and EOF still terminate the transport. The monitor now
  preserves those reasons through log rotation for the next field session.
  Do not treat the final five-character paste as a successful transcript; a
  new installed-binary pause-and-resume session must verify the fix.
