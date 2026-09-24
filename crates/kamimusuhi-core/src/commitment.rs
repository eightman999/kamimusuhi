//! Recognising a reply that takes on work for later.
//!
//! A Persona reply such as 「調べてみますね」 promises work that outlives the
//! turn. Saying so starts nothing: only a task on the task plane does. The
//! host uses this check to notice a promise that was not backed by a task,
//! so it can ask the Persona once to delegate (or to stop promising) and,
//! failing that, put the promise on the board for the operator.
//!
//! This is a lexical heuristic over the reply text. It never grants
//! authority or starts work by itself; a false positive costs one host
//! nudge and one board entry the operator can dismiss.

/// Phrases that commit to doing something after this reply.
const PROMISES: &[&str] = &[
    "調べてみます",
    "調べます",
    "調べておきます",
    "調べてきます",
    "調査します",
    "調査してみます",
    "調査を開始",
    "確認します",
    "確認してみます",
    "確認しておきます",
    "確認してきます",
    "検証してみます",
    "検証します",
    "取り掛かります",
    "とりかかります",
    "着手します",
    "進めておきます",
    "進めてみます",
    "やってみます",
    "試してみます",
    "作業を開始",
    "作業を始め",
    "依頼してみます",
    "依頼します",
    "任せてみます",
    "報告します",
    "ご報告します",
    "お知らせします",
    "お伝えします",
    "少々お待ち",
    "しばらくお待ち",
    "お待ちください",
    "i'll look into",
    "i will look into",
    "i'll investigate",
    "i will investigate",
    "i'll check",
    "let me look into",
    "let me check",
    "i'll get back",
    "working on it",
];

/// Clauses that make a sentence an offer or a condition, not a promise
/// (「必要なら調べます」「よろしければ確認します」).
const CONDITIONS: &[&str] = &[
    "なら",
    "ければ",
    "であれば",
    "でしたら",
    "場合は",
    "もし",
    "if you",
    "would you",
];

/// Whether `reply` commits to work that continues after the reply.
///
/// Questions and conditional offers do not count; the promise must be a
/// plain statement in at least one sentence.
pub fn promises_background_work(reply: &str) -> bool {
    sentences(reply).any(|(sentence, question)| {
        if question {
            return false;
        }
        let lower = sentence.to_lowercase();
        let trimmed = lower.trim_end_matches(['ね', 'よ', '♪', '～', '〜', ' ']);
        if trimmed.ends_with('か') {
            return false;
        }
        PROMISES.iter().any(|p| lower.contains(p)) && !CONDITIONS.iter().any(|c| lower.contains(c))
    })
}

/// Sentences of `text` with whether each ended in a question mark.
fn sentences(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        loop {
            if rest.is_empty() {
                return None;
            }
            let end = rest
                .char_indices()
                .find(|(_, c)| matches!(c, '。' | '！' | '？' | '!' | '?' | '\n'));
            let (sentence, question) = match end {
                Some((i, c)) => {
                    let sentence = &rest[..i];
                    rest = &rest[i + c.len_utf8()..];
                    (sentence, matches!(c, '？' | '?'))
                }
                None => {
                    let sentence = rest;
                    rest = "";
                    (sentence, false)
                }
            };
            if !sentence.trim().is_empty() {
                return Some((sentence, question));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_promise_is_recognised() {
        assert!(promises_background_work(
            "承知いたしました。私の記憶アーキテクチャに「忘却」の仕組みを導入して効率化できるか、Devinのswe2を使って調べてみますね。"
        ));
        assert!(promises_background_work("では確認しておきます"));
        assert!(promises_background_work("Sure, I'll look into it."));
        assert!(promises_background_work("結果が出たらお知らせします。"));
    }

    #[test]
    fn answers_questions_and_offers_are_not_promises() {
        assert!(!promises_background_work("忘却は記憶の整理に役立ちます。"));
        assert!(!promises_background_work(
            "調べました。結果は次の通りです。"
        ));
        assert!(!promises_background_work("Devinで調べますか？"));
        assert!(!promises_background_work("こちらで調べましょうか"));
        assert!(!promises_background_work("必要なら調べます。"));
        assert!(!promises_background_work("よろしければ確認してみます。"));
        assert!(!promises_background_work("まだ着手していません。"));
        assert!(!promises_background_work(""));
    }
}
