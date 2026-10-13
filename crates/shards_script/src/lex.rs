//! Shell-like words for one scenario line.

/// The words of a line and the part of the line that held them.
#[derive(Debug, PartialEq, Eq)]
pub struct Words {
    pub words: Vec<String>,
    /// The line up to a trailing `# comment`, trimmed.
    pub code: String,
}

/// Splits a line the way a shell would, without expansions:
///
/// - words are separated by whitespace; `#` at the start of a word begins a comment;
/// - `'…'` keeps everything literal (use it for JSON);
/// - `"…"` keeps spaces, with `\"` and `\\` as escapes;
/// - outside quotes a backslash makes the next character literal;
/// - quoted and bare pieces glue together: `text="a b"c` is one word `text=a bc`.
pub fn split_words(line: &str) -> Result<Words, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut code_end = line.len();
    let mut chars = line.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '#' if !in_word => {
                code_end = at;
                break;
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some((_, '\'')) => break,
                        Some((_, c)) => word.push(c),
                        None => return Err("unclosed `'` quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some((_, '"')) => break,
                        Some((_, '\\')) => match chars.next() {
                            Some((_, c @ ('"' | '\\'))) => word.push(c),
                            Some((_, c)) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unclosed `\"` quote".into()),
                        },
                        Some((_, c)) => word.push(c),
                        None => return Err("unclosed `\"` quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.push(chars.next().map_or('\\', |(_, c)| c));
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(Words { words, code: line[..code_end].trim().to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        split_words(line).unwrap().words
    }

    #[test]
    fn plain_words_and_comments() {
        assert_eq!(words("act attack target=e6  repeat=true"), ["act", "attack", "target=e6", "repeat=true"]);
        assert_eq!(words("   "), Vec::<String>::new());
        assert_eq!(words("# only a comment"), Vec::<String>::new());
        let w = split_words("step 60   # let it settle").unwrap();
        assert_eq!((w.words, w.code.as_str()), (vec!["step".to_owned(), "60".to_owned()], "step 60"));
        // A `#` inside a word is part of it: `field:e5:Health.current<30` has none, but ids may.
        assert_eq!(words("act say text=a#b"), ["act", "say", "text=a#b"]);
        assert_eq!(words(r"act say text=\#tag"), ["act", "say", "text=#tag"]);
    }

    #[test]
    fn quotes_keep_spaces_and_glue_pieces() {
        assert_eq!(words(r#"expect journal "e6 phase 1->2""#), ["expect", "journal", "e6 phase 1->2"]);
        assert_eq!(words("expect journal 'it said \"hi\" # not a comment'"), ["expect", "journal", "it said \"hi\" # not a comment"]);
        assert_eq!(words(r#"act say text="a b"c"#), ["act", "say", "text=a bc"]);
        assert_eq!(words(r#"expect result contains """#), ["expect", "result", "contains", ""]);
        assert_eq!(words(r#"expect result contains "a \"b\" \\ \n""#), ["expect", "result", "contains", r#"a "b" \ \n"#]);
    }

    #[test]
    fn json_survives_single_quotes() {
        let line = r#"act set '{"id":"e6","component":"Health","path":"current","value":1900}'"#;
        let w = words(line);
        assert_eq!(w.len(), 3);
        assert_eq!(w[2], r#"{"id":"e6","component":"Health","path":"current","value":1900}"#);
    }

    #[test]
    fn unclosed_quotes_are_reported() {
        assert_eq!(split_words("expect journal \"oops").unwrap_err(), "unclosed `\"` quote");
        assert_eq!(split_words("expect journal 'oops").unwrap_err(), "unclosed `'` quote");
    }
}
