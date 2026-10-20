//! Font spans in a text (U1): `[font=symbols]word[/font]` draws `word` in the theme's font
//! `symbols` ([`ThemeDef::fonts`](crate::ThemeDef::fonts)), the rest of the line in the theme's
//! own. Square brackets survive Fluent, which owns `{...}`, so a language's message can carry them.
//!
//! - **Where.** The value of a `Text` or `Hint` widget, from content, a message (`@id`) or a source.
//!   Other texts (labels, titles) are drawn as written.
//! - **Nesting.** `[font=a]x[font=b]y[/font]z[/font]`: `y` in `b`, `x` and `z` in `a`.
//! - **What is not markup stays text.** A tag that is never closed, a `[/font]` with nothing open, and
//!   a name that is not letters, digits, `_` and `-` (`[font=]`, `[font=a b]`) are drawn as written.
//! - **Who reads it without markup.** Screen readers, `ui_read` and `observe` get the text without
//!   the tags ([`plain`]): an agent reads `the door said ᚱᚢᚾ`, not the markup.

use std::borrow::Cow;

/// A run of text drawn in one font: `None` is the theme's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span<'a> {
    pub font: Option<&'a str>,
    pub text: Cow<'a, str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token<'a> {
    Text(&'a str),
    /// `[font=name]`, as written, and its name.
    Open(&'a str, &'a str),
    /// `[/font]`.
    Close(&'a str),
}

const CLOSE: &str = "[/font]";
const OPEN: &str = "[font=";

/// Whether `name` can name a font: letters, digits, `_` and `-`.
fn good_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut plain_from = 0;
    let mut at = 0;
    while let Some(i) = rest.find('[') {
        let tag = &rest[i..];
        let found = if tag.starts_with(CLOSE) {
            Some((Token::Close(&tag[..CLOSE.len()]), CLOSE.len()))
        } else if let Some(after) = tag.strip_prefix(OPEN)
            && let Some(end) = after.find(']')
            && good_name(&after[..end])
        {
            let len = OPEN.len() + end + 1;
            Some((Token::Open(&tag[..len], &after[..end]), len))
        } else {
            None
        };
        let start = at + i;
        match found {
            Some((token, len)) => {
                if start > plain_from {
                    out.push(Token::Text(&text[plain_from..start]));
                }
                out.push(token);
                at = start + len;
                plain_from = at;
            }
            None => at = start + 1,
        }
        rest = &text[at..];
    }
    if plain_from < text.len() {
        out.push(Token::Text(&text[plain_from..]));
    }
    out
}

/// The runs of `text` and the font of each, in order; neighbours in one font are one run, and no run
/// is empty. Text without markup is one run in the theme's font (none for an empty text).
pub fn spans(text: &str) -> Vec<Span<'_>> {
    let tokens = tokens(text);
    // Which tags pair up: a close takes the nearest open before it that is still open.
    let mut paired = vec![false; tokens.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Open(..) => open.push(i),
            Token::Close(_) => {
                if let Some(o) = open.pop() {
                    paired[o] = true;
                    paired[i] = true;
                }
            }
            Token::Text(_) => {}
        }
    }
    let mut out: Vec<Span> = Vec::new();
    let mut fonts: Vec<&str> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let text = match (*token, paired[i]) {
            (Token::Open(_, name), true) => {
                fonts.push(name);
                continue;
            }
            (Token::Close(_), true) => {
                fonts.pop();
                continue;
            }
            (Token::Text(t) | Token::Open(t, _) | Token::Close(t), _) => t,
        };
        let font = fonts.last().copied();
        match out.last_mut() {
            Some(last) if last.font == font => last.text.to_mut().push_str(text),
            _ => out.push(Span { font, text: Cow::Borrowed(text) }),
        }
    }
    out
}

/// `text` without its markup: what screen readers and agents read.
pub fn plain(text: &str) -> Cow<'_, str> {
    if !text.contains('[') {
        return Cow::Borrowed(text);
    }
    let spans = spans(text);
    match spans.as_slice() {
        [] => Cow::Borrowed(""),
        [one] => one.text.clone(),
        many => Cow::Owned(many.iter().map(|s| s.text.as_ref()).collect()),
    }
}

/// Whether `text` has markup that changes a font.
pub fn has_spans(text: &str) -> bool {
    text.contains(OPEN) && spans(text).iter().any(|s| s.font.is_some())
}

/// The fonts `text` names in tags that pair up, each once, in order.
pub fn fonts_named(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for span in spans(text) {
        if let Some(font) = span.font
            && !out.contains(&font)
        {
            out.push(font);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(text: &str) -> Vec<(Option<&str>, String)> {
        spans(text).into_iter().map(|s| (s.font, s.text.into_owned())).collect()
    }

    #[test]
    fn a_tag_draws_its_words_in_its_font_and_the_rest_in_the_themes() {
        assert_eq!(runs("the door said [font=symbols]ᚱᚢᚾ[/font]."), [(None, "the door said ".into()), (Some("symbols"), "ᚱᚢᚾ".into()), (None, ".".into())]);
        assert_eq!(runs("no markup [x] here"), [(None, "no markup [x] here".into())]);
        assert_eq!(runs(""), []);
        assert_eq!(runs("[font=a][/font]"), [], "an empty span is no run");
    }

    #[test]
    fn tags_nest_and_neighbours_in_one_font_are_one_run() {
        assert_eq!(runs("[font=a]x[font=b]y[/font]z[/font]w"), [(Some("a"), "x".into()), (Some("b"), "y".into()), (Some("a"), "z".into()), (None, "w".into())]);
        assert_eq!(runs("[font=a]x[/font][font=a]y[/font]"), [(Some("a"), "xy".into())]);
    }

    #[test]
    fn what_is_not_markup_stays_text() {
        // Never closed: the tag is text, and so are its words, in the font around it.
        assert_eq!(runs("a [font=symbols]b"), [(None, "a [font=symbols]b".into())]);
        // A close with nothing open.
        assert_eq!(runs("a[/font]b"), [(None, "a[/font]b".into())]);
        // Names that are not names.
        assert_eq!(runs("[font=]x[/font]"), [(None, "[font=]x[/font]".into())]);
        assert_eq!(runs("[font=a b]x[/font]"), [(None, "[font=a b]x[/font]".into())]);
        // The inner open pairs with the only close: the outer stays text.
        assert_eq!(runs("[font=a]x[font=b]y[/font]"), [(None, "[font=a]x".into()), (Some("b"), "y".into())]);
    }

    #[test]
    fn plain_text_and_the_fonts_named() {
        assert_eq!(plain("the door said [font=symbols]ᚱᚢᚾ[/font]."), "the door said ᚱᚢᚾ.");
        assert_eq!(plain("a [font=symbols]b"), "a [font=symbols]b");
        assert!(matches!(plain("nothing"), Cow::Borrowed("nothing")));
        assert_eq!(fonts_named("[font=a]x[font=b]y[/font][/font] [font=a]z[/font] [font=c]"), ["a", "b"]);
        assert!(has_spans("x [font=a]y[/font]") && !has_spans("x [font=a]y") && !has_spans("[x]"));
    }
}
