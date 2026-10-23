// Selection context for one actual QA request. Kept independent of recorder,
// provider and platform IO so a follow-up can be checked without live speech.
pub(super) fn question_user_content(
    is_first_turn: bool,
    last_answered_selection: Option<&str>,
    selection: Option<&str>,
    question: &str,
) -> String {
    let selected = selection.unwrap_or_default().trim();
    let changed = last_answered_selection.map(str::trim) != Some(selected);
    if !selected.is_empty() && (is_first_turn || changed) {
        format!("# 选区原文\n{selected}\n\n# 我的问题\n{question}")
    } else {
        question.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::question_user_content;

    #[test]
    fn first_question_contains_the_actual_selection() {
        assert_eq!(question_user_content(true, None, Some(" 第一段资料 "), "解释一下"),
            "# 选区原文\n第一段资料\n\n# 我的问题\n解释一下");
    }

    #[test]
    fn changed_selection_is_sent_with_followup_question() {
        assert_eq!(question_user_content(false, Some("旧资料"), Some("新的合同条款"), "这一条呢？"),
            "# 选区原文\n新的合同条款\n\n# 我的问题\n这一条呢？");
    }

    #[test]
    fn first_selection_after_an_unselected_question_is_sent() {
        assert_eq!(question_user_content(false, None, Some("New reference text"), "What does this mean?"),
            "# 选区原文\nNew reference text\n\n# 我的问题\nWhat does this mean?");
    }

    #[test]
    fn unchanged_selection_is_not_repeated_in_each_turn() {
        assert_eq!(question_user_content(false, Some("参考正文"), Some(" 参考正文\n"), "再举个例子"), "再举个例子");
    }

    #[test]
    fn unselected_followup_keeps_existing_chat_context() {
        for selection in [None, Some(" \n ")] {
            assert_eq!(question_user_content(false, Some("参考正文"), selection, "继续"), "继续");
        }
    }
}
