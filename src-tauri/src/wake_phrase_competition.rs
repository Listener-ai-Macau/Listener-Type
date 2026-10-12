// Included in wake_phrase::platform. A competing recognition is affirmative
// acoustic evidence, not an absent-transcript count or a stricter threshold.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AcousticKeywordConflict {
    pub keyword_end_seconds: f32,
    pub alternative_end_seconds: f32,
    pub shared_anchor_seconds: Vec<f32>,
}

#[derive(Debug)]
struct TimedKeywordPath {
    label: String,
    tokens: Vec<String>,
    times: Vec<f32>,
}

fn chinese_recognition_units(text: &str) -> Vec<char> {
    text.chars().filter(|ch| ch.to_pinyin().is_some()).collect()
}

// Both raw and enhanced speech recognition must supply the same complete
// competing pronunciation. A shorter prefix, silence or mere absence supplies
// none. Do not manufacture a missing final syllable from the configured label.
fn agreed_competing_phrases(raw: &str, enhanced: &str, phrase: &str) -> Vec<String> {
    let Ok(phrase) = super::normalize_configured_phrase(phrase) else { return Vec::new() };
    let expected = phrase.chars().collect::<Vec<_>>();
    if expected.len() < 2 { return Vec::new(); }
    let raw = chinese_recognition_units(raw);
    let enhanced = chinese_recognition_units(enhanced);
    let expected_phones = phrase_tokens_with_tone(&phrase, false).unwrap_or_default();
    let first = phrase_tokens_with_tone(&expected[0].to_string(), false).unwrap_or_default();
    let enhanced_windows = enhanced.windows(expected.len()).map(|window| {
        phrase_tokens_with_tone(&window.iter().collect::<String>(), false).unwrap_or_default()
    }).collect::<Vec<_>>();
    let mut result = Vec::new();
    for window in raw.windows(expected.len()) {
        let candidate = window.iter().collect::<String>();
        let Ok(phones) = phrase_tokens_with_tone(&candidate, false) else { continue };
        if phones == expected_phones || !phones.starts_with(&first)
            || !enhanced_windows.contains(&phones) || result.contains(&candidate) { continue; }
        result.push(candidate);
    }
    result
}

struct KeywordComparisonStream<'a> {
    runtime: &'a Runtime,
    stream: *const c_void,
}
impl Drop for KeywordComparisonStream<'_> {
    fn drop(&mut self) { unsafe { (self.runtime.destroy_stream)(self.stream) }; }
}

fn first_timed_keyword(runtime: &Runtime, samples: &[f32], keywords: Option<&str>) -> Result<Option<TimedKeywordPath>, String> {
    let keyword_string = keywords.map(|value| CString::new(value).map_err(|_| "invalid comparison keywords")).transpose()?;
    let stream = unsafe { match keyword_string.as_ref() {
        Some(value) => (runtime.create_stream_with_keywords)(runtime.spotter, value.as_ptr()),
        None => (runtime.create_stream)(runtime.spotter),
    } };
    if stream.is_null() { return Err("create keyword comparison stream failed".into()); }
    let guard = KeywordComparisonStream { runtime, stream };
    let duration = samples.len() as f32 / SAMPLE_RATE as f32;
    let read_ready = || -> Option<TimedKeywordPath> {
        unsafe {
            while (runtime.is_ready)(runtime.spotter, guard.stream) != 0 {
                (runtime.decode)(runtime.spotter, guard.stream);
                let ptr = (runtime.get_result)(runtime.spotter, guard.stream);
                if ptr.is_null() { continue; }
                let r = &*ptr;
                let found = if !r.keyword.is_null() && !CStr::from_ptr(r.keyword).to_bytes().is_empty()
                    && r.count > 0 && !r.tokens_arr.is_null() && !r.timestamps.is_null()
                    && r.start_time.is_finite() && r.start_time >= 0.0 {
                    let mut tokens = Vec::new();
                    let mut times = Vec::new();
                    for index in 0..r.count as usize {
                        let token = *r.tokens_arr.add(index);
                        if token.is_null() { break; }
                        tokens.push(CStr::from_ptr(token).to_string_lossy().into_owned());
                        times.push(r.start_time + *r.timestamps.add(index));
                    }
                    (tokens.len() == r.count as usize && times.iter().all(|t| t.is_finite() && *t >= 0.0 && *t <= duration)
                        && times.windows(2).all(|t| t[0] <= t[1]))
                        .then(|| TimedKeywordPath { label: CStr::from_ptr(r.keyword).to_string_lossy().into_owned(), tokens, times })
                } else { None };
                (runtime.destroy_result)(ptr);
                if found.is_some() { return found; }
            }
        }
        None
    };
    for chunk in samples.chunks(800) {
        unsafe { (runtime.accept_waveform)(guard.stream, SAMPLE_RATE, chunk.as_ptr(), chunk.len() as i32) };
        if let Some(found) = read_ready() { return Ok(Some(found)); }
    }
    let padding = vec![0.0f32; FINAL_PADDING_SAMPLES];
    unsafe {
        (runtime.accept_waveform)(guard.stream, SAMPLE_RATE, padding.as_ptr(), padding.len() as i32);
        (runtime.input_finished)(guard.stream);
    }
    Ok(read_ready())
}

fn same_acoustic_anchor(keyword: &TimedKeywordPath, alternative: &TimedKeywordPath,
    phrase: &str, claimed_end: f32) -> Option<AcousticKeywordConflict> {
    if keyword.label != phrase || alternative.label == phrase || !claimed_end.is_finite()
        || claimed_end <= 0.0 || keyword.times.is_empty() || alternative.times.is_empty() { return None; }
    let keyword_end = *keyword.times.last()?;
    // Coordinates are integer PCM sample positions. No overlap tolerance,
    // repaired observation clock or unrelated later occurrence can qualify.
    if (keyword_end * SAMPLE_RATE as f32).round() as u64 != (claimed_end * SAMPLE_RATE as f32).round() as u64 { return None; }
    let prefix = keyword.tokens.iter().zip(&alternative.tokens).take_while(|(a,b)| a == b).count();
    let first = phrase.chars().next()?.to_string();
    let first_phone_count = phrase_tokens_with_tone(&first, false).ok()?.len();
    if prefix < first_phone_count || prefix == keyword.tokens.len() { return None; }
    let anchor = keyword.times.get(..prefix)?;
    let other_anchor = alternative.times.get(..prefix)?;
    if !anchor.iter().zip(other_anchor).all(|(a,b)|
        (*a * SAMPLE_RATE as f32).round() as u64 == (*b * SAMPLE_RATE as f32).round() as u64) { return None; }
    Some(AcousticKeywordConflict { keyword_end_seconds: keyword_end,
        alternative_end_seconds: *alternative.times.last()?, shared_anchor_seconds: anchor.to_vec() })
}

pub fn compare_recognized_keyword_paths(pcm: &[u8], phrase: &str, raw: &str,
    enhanced: &str, claimed_end: f32) -> Result<Option<AcousticKeywordConflict>, String> {
    let alternatives = agreed_competing_phrases(raw, enhanced, phrase);
    if alternatives.is_empty() { return Ok(None); }
    let mut normalizer = StreamingNormalizer::default();
    let mut samples = Vec::new();
    for chunk in pcm.chunks(1600) { samples.extend(normalizer.accept(chunk)?); }
    samples.extend(normalizer.finish());
    let (score, threshold) = configured_keyword_values(phrase);
    let baseline = load_with_config(phrase, score, threshold, false)?;
    let Some(keyword) = first_timed_keyword(&baseline, &samples, None)? else { return Ok(None) };
    let mut keywords = String::new();
    for candidate in &alternatives {
        for tone in [true, false] {
            if !keywords.is_empty() { keywords.push('\n'); }
            keywords.push_str(&keyword_entry(candidate, candidate, score, threshold, tone)?);
        }
    }
    let Some(winner) = first_timed_keyword(&baseline, &samples, Some(&keywords.replace('\n', "/")))? else { return Ok(None) };
    if !alternatives.contains(&winner.label) { return Ok(None); }
    Ok(same_acoustic_anchor(&keyword, &winner, phrase, claimed_end))
}

#[cfg(test)]
mod competition_tests {
    use super::*;
    #[test]
    fn conflict_requires_complete_agreement_and_the_same_acoustic_occurrence() {
        assert_eq!(agreed_competing_phrases("从从零开始到一", "从零开始到一", "开始录音"), ["开始到一"]);
        assert!(agreed_competing_phrases("从零开始到", "从零开始到", "开始录音").is_empty());
        assert!(agreed_competing_phrases("开始到一", "开始录音", "开始录音").is_empty());
        let keyword = TimedKeywordPath { label: "开始录音".into(), tokens: vec!["k","ai","sh","i","l","u","y","in"].into_iter().map(String::from).collect(), times: vec![0.8,0.84,1.0,1.04,1.3,1.4,1.6,1.76] };
        let mut alternate = TimedKeywordPath { label:"开始到一".into(), tokens:vec!["k","ai","sh","i","d","ao","y","i"].into_iter().map(String::from).collect(), times:vec![0.8,0.84,1.0,1.04,1.3,1.4,1.6,1.72] };
        assert!(same_acoustic_anchor(&keyword,&alternate,"开始录音",1.76).is_some());
        assert!(same_acoustic_anchor(&keyword,&alternate,"开始录音",2.76).is_none());
        alternate.times[0] = 0.88;
        assert!(same_acoustic_anchor(&keyword,&alternate,"开始录音",1.76).is_none());
    }

    #[test]
    #[ignore = "one retained actual false wake; cached native runtime, no cloud"]
    fn saved_ordinary_speech_selects_an_acoustically_bound_competitor() {
        let path = std::env::var("LISTENER_WAKE_DIAG_CLIP").expect("retained failure");
        let wav = fs::read(path).unwrap();
        let pcm = denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
        let result = compare_recognized_keyword_paths(&pcm,"开始录音","从从零开始到一","从从零开始到一",1.76).unwrap();
        println!("saved_keyword_competition {result:?}");
        assert!(result.is_some(), "same acoustic source must refute the wrong complete command");
    }
}
