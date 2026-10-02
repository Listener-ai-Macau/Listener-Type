//! Shared content coordinates for provisional transcripts and editor delivery.

pub(crate) fn is_decorative(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。' | '、' | '；' | '：' | '？' | '！' | '“' | '”' | '‘' | '’'
                | '（' | '）' | '【' | '】' | '《' | '》' | '…' | '—'
        )
}

pub(crate) fn content_key(text: &str) -> String {
    text.chars()
        .filter(|ch| !is_decorative(*ch))
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn terminal_punctuation_only(text: &str) -> bool {
    let mut terminal = false;
    for ch in text.chars() {
        if matches!(ch, '。' | '.' | '！' | '!' | '？' | '?' | '…') {
            terminal = true;
        } else if !ch.is_whitespace() && !matches!(ch, '”' | '’' | '"' | '\'' | '）' | ')' | '】' | ']') {
            return false;
        }
    }
    terminal
}

/// Map the terminal decoration of an already covered body. A late two-pass
/// spelling correction may preserve that boundary without permitting any
/// rewrite or new word to cross the irreversible delivery seam.
pub(crate) fn revised_terminal_suffix<'a>(text: &'a str, consumed_key: &str) -> Option<&'a str> {
    let final_key = content_key(text);
    if final_key != consumed_key {
        let consumed: Vec<char> = consumed_key.chars().collect();
        if !(8..=512).contains(&consumed.len()) { return None; }
        let final_content: Vec<char> = final_key.chars().collect();
        let anchor = &consumed[consumed.len() - 8..];
        let max_edits = (consumed.len() / 5).clamp(2, 12);
        if consumed.len().abs_diff(final_content.len()) > max_edits { return None; }
        let retained_anchor = final_content.ends_with(anchor)
            && final_content.windows(anchor.len()).filter(|window| *window == anchor).count() == 1;
        if !retained_anchor && consumed.last() != final_content.last() { return None; }
        let mut previous: Vec<usize> = (0..=final_content.len()).collect();
        let mut current = vec![0; final_content.len() + 1];
        for (index, expected) in consumed.iter().enumerate() {
            current[0] = index + 1;
            for prefix in 1..=final_content.len() {
                current[prefix] = (previous[prefix] + 1)
                    .min(current[prefix - 1] + 1)
                    .min(previous[prefix - 1] + usize::from(*expected != final_content[prefix - 1]));
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let terminal_cost = previous[final_content.len()];
        if terminal_cost > max_edits { return None; }
        if !retained_anchor {
            // A correction inside the old suffix can remove that exact
            // anchor. The complete consumed body must then align uniquely
            // to this terminal position, with the last content unit retained.
            // A cheaper or equally good earlier seam means possible new
            // speech, so it cannot authorize punctuation-only coverage.
            let first_possible_seam = consumed.len().saturating_sub(max_edits);
            if previous[first_possible_seam..final_content.len()]
                    .iter().any(|cost| *cost <= terminal_cost)
            { return None; }
        }
    }
    let (offset, last_content) = text.char_indices().rev().find(|(_, ch)| !is_decorative(*ch))?;
    Some(&text[offset + last_content.len_utf8()..])
}

/// Locate an already consumed prefix. Only case/formatting differences are
/// tolerated; a changed word, a shortened prefix, or a shifted opening fails.
/// Decorations at that committed seam belong to the prefix, not the new body.
pub(crate) fn consumed_prefix_end(text: &str, key: &str) -> Option<usize> {
    if key.is_empty() {
        return Some(0);
    }
    let mut expected = key.chars();
    let mut next = expected.next();
    for (offset, ch) in text.char_indices() {
        if is_decorative(ch) {
            continue;
        }
        if next.is_none() {
            return Some(offset);
        }
        for lower in ch.to_lowercase() {
            if next != Some(lower) {
                return None;
            }
            next = expected.next();
        }
    }
    next.is_none().then_some(text.len())
}

/// Locate the end of a revised, already consumed prefix in an owner-approved
/// snapshot. The editor's submitted words remain immutable: this only maps
/// their source coordinate so genuinely new words can continue after it.
/// Require retained context and a unique best alignment, including competing
/// terminal seams, rather than choosing a later equally plausible boundary.
pub(crate) fn revised_prefix_end(text: &str, consumed_key: &str) -> Option<usize> {
    let consumed: Vec<char> = consumed_key.chars().collect();
    if consumed.len() > 512 {
        return None;
    }
    let max_edits = (consumed.len() / 5).clamp(1, 12);
    if consumed.len().saturating_sub(max_edits) < 4 {
        return None;
    }
    let mut content = Vec::new();
    let mut ends = Vec::new();
    for (offset, ch) in text.char_indices() {
        if is_decorative(ch) {
            continue;
        }
        for lower in ch.to_lowercase() {
            content.push(lower);
            ends.push(offset + ch.len_utf8());
        }
    }
    // An insertion inside the old body is not independent proof of a new
    // continuation, even when it can be aligned to an earlier prefix.
    if content.ends_with(&consumed[consumed.len() - 4..]) {
        return None;
    }
    let first = consumed.len().saturating_sub(max_edits);
    let last = content.len().min(consumed.len() + max_edits);
    if first > last {
        return None;
    }
    let mut previous: Vec<usize> = (0..=last).collect();
    let mut current = vec![0; last + 1];
    for (index, expected) in consumed.iter().enumerate() {
        current[0] = index + 1;
        for prefix in 1..=last {
            current[prefix] = (previous[prefix] + 1)
                .min(current[prefix - 1] + 1)
                .min(previous[prefix - 1] + usize::from(*expected != content[prefix - 1]));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let cost = *previous[first..=last].iter().min()?;
    if cost > max_edits {
        return None;
    }
    let mut seams = (first..=last).filter(|&prefix| previous[prefix] == cost);
    let boundary = seams.next()?;
    if seams.next().is_some()
        || boundary == 0
        || boundary >= content.len()
        || content[boundary - 1] != *consumed.last()?
        || ends[boundary - 1] == ends[boundary]
    {
        return None;
    }
    Some(ends[boundary - 1])
}

fn stability_units(text: &str) -> Vec<(char, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut units = Vec::new();
    for (index, &(offset, ch)) in chars.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| chars[i].1);
        let next = chars.get(index + 1).map(|(_, ch)| *ch);
        // Decimal separators and literal operators belong to the body. Word
        // spacing also distinguishes e.g. "no where" from "nowhere".
        let numeric_separator = matches!(ch, '.' | ',' | '，' | '。' | ':' | '：')
            && previous.is_some_and(|ch| ch.is_ascii_digit())
            && next.is_some_and(|ch| ch.is_ascii_digit());
        let literal_operator = ch.is_ascii_punctuation()
            && !matches!(ch, ',' | '.' | '!' | '?' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>');
        let word_space = ch.is_whitespace()
            && previous.is_some_and(|ch| ch.is_ascii_alphabetic())
            && next.is_some_and(|ch| ch.is_ascii_alphabetic());
        if is_decorative(ch) && !numeric_separator && !literal_operator && !word_space {
            continue;
        }
        for lower in ch.to_lowercase() {
            units.push((lower, offset));
        }
    }
    units
}

/// Stability belongs to spoken content. Once all retained revisions agree
/// on that content, use the newest clause punctuation without waiting for it
/// to age again. No new or revised body word can cross the stable boundary.
pub(crate) fn stable_content_prefix<'a>(
    current: &str,
    revisions: impl Iterator<Item = &'a str>,
) -> String {
    let units = stability_units(current);
    let mut count = units.len();
    for revision in revisions {
        let older = stability_units(revision);
        count = units.iter().take(count).zip(older.iter())
            .take_while(|(left, right)| left.0 == right.0).count();
    }
    // Never split a Unicode character whose lowercase mapping has several
    // comparison units.
    while count > 0 && count < units.len() && units[count - 1].1 == units[count].1 {
        count -= 1;
    }
    if count == 0 {
        return String::new();
    }
    let end = units.get(count).map_or(current.len(), |(_, offset)| *offset);
    current[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumed_boundary_uses_the_shared_content_coordinate() {
        let key = content_key("确认 NC 的结果。");
        let revised = "确认 nc 的结果，后续继续。";
        let end = consumed_prefix_end(revised, &key).unwrap();
        assert_eq!(&revised[end..], "后续继续。");
        assert_eq!(consumed_prefix_end("确认 nc 的结果。", &key), Some("确认 nc 的结果。".len()));
        assert_eq!(consumed_prefix_end("确认另外的结果。后续继续。", &key), None);
        assert_eq!(consumed_prefix_end("确认 nc", &key), None);
        assert_eq!(consumed_prefix_end("前面插入确认 nc 的结果。", &key), None);
    }

    #[test]
    fn consumed_boundary_preserves_unicode_byte_offsets() {
        let text = "İ 已确认。新的正文。";
        let end = consumed_prefix_end(text, &content_key("İ 已确认。")).unwrap();
        assert_eq!(&text[end..], "新的正文。");
    }

    #[test]
    fn revised_prefix_maps_short_corrected_openings_and_small_growth() {
        let key = content_key("那里看一下先");
        for (text, tail) in [
            ("那你看一下先优化速度。", "优化速度。"),
            ("那你看一下先，继续。", "，继续。"),
            ("那里看一下先好。", "好。"),
        ] {
            let end = revised_prefix_end(text, &key).unwrap();
            assert_eq!(&text[end..], tail);
        }
        let text = "İ 那你看一下先，继续说话。";
        let end = revised_prefix_end(text, &content_key("İ 那里看一下先")).unwrap();
        assert_eq!(&text[end..], "，继续说话。");
    }

    #[test]
    fn revised_prefix_rejects_ambiguous_or_unproved_continuations() {
        for (consumed, text) in [
            ("aaaaab", "aaaabbbc"), // Both five and six units cost one edit.
            ("那里看一下先", "完全不同的任务继续说话。"),
            ("那里", "那你继续说话。"), // Too little retained context.
            ("那里看一下先", "那你看一下先。"), // Only a correction, no new body.
            ("那里看一下先", "那里现在看一下先。"), // Internal insertion only.
            ("那里看一下先", "那你看一下后优化速度。"), // The old seam was also rewritten.
            ("那里看一下先", "先把那里看一下先优化速度。"), // Shifted opening exceeds the budget.
            ("abcdxi", "abcdİ继续。"), // A seam inside the multi-unit lowercase of İ is invalid.
        ] {
            assert_eq!(revised_prefix_end(text, &content_key(consumed)), None, "{consumed} -> {text}");
        }
    }

    #[test]
    fn stable_body_accepts_late_clause_punctuation_without_reaging_words() {
        assert_eq!(stable_content_prefix("第一句话？接下来", ["第一句话", "第一句话？接"].into_iter()), "第一句话？");
        assert_eq!(stable_content_prefix("确认 nc 的结果，继续下一句。", ["确认 NC 的结果。继续下一句", "确认 nc 的结果，继续下一句"].into_iter()), "确认 nc 的结果，继续下一句。");
        assert_eq!(stable_content_prefix("今天谈论速度。", ["今天讨论速度"].into_iter()), "今天");
    }

    #[test]
    fn terminal_boundary_survives_a_revision_inside_the_old_suffix_anchor() {
        let consumed = content_key("我们先检查这个流程然后看下怎么处理");
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个流程然后看一下怎么处理。", &consumed), Some("。"));
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个系统然后看一下怎么处理。", &consumed), Some("。"));
        assert_eq!(revised_terminal_suffix(
            "我们先检查这个流程然后看下怎么处理，然后继续处理。", &consumed), None);
        assert_eq!(revised_terminal_suffix(
            "完全不同的任务现在还需要知道如何处理。", &consumed), None);
    }

    #[test]
    fn stability_preserves_numeric_literals_word_boundaries_and_unicode() {
        for (current, earlier, expected) in [
            ("价格 1.2。", "价格 12。", "价格 1"),
            ("比例 1/2。", "比例 12。", "比例 1"),
            ("时间 12:30。", "时间 1230。", "时间 12"),
            ("no where。", "nowhere。", "no"),
            ("İ 后续。", "i 后续。", ""),
        ] {
            assert_eq!(stable_content_prefix(current, [earlier].into_iter()), expected);
        }
    }
}
