use deadbeef_sys::ddb_action_context_t;


/// Parse lines like: `"Ctrl k" 0 0 toggle_stop_after_album`
///
/// Returns (keystroke, is_global, action_name)
pub fn parse_line(line: &str) -> Result<(String, bool, String, ddb_action_context_t), String> {
    let s = line.trim();
    let rest = s
        .strip_prefix('"')
        .ok_or_else(|| "line must start with a double quote for keystroke".to_string())?;

    let end_quote_idx = rest
        .find('"')
        .ok_or_else(|| "missing closing quote for keystroke".to_string())?;
    let keystroke = &rest[..end_quote_idx];

    let after = rest[end_quote_idx + 1..].trim();
    let mut parts = after.split_whitespace();

    // second number -> is_global
    let num2 = parts
        .next()
        .ok_or_else(|| "missing second number".to_string())?;
    let ctx = match num2.parse::<ddb_action_context_t>() {
        Ok(n) => n,
        Err(_) => return Err("second number is not a valid integer".to_string()),
    };

    // second number -> is_global
    let num2 = parts
        .next()
        .ok_or_else(|| "missing second number".to_string())?;
    let is_global = match num2.parse::<i64>() {
        Ok(n) => n != 0,
        Err(_) => return Err("second number is not a valid integer".to_string()),
    };

    let action_tokens: Vec<&str> = parts.collect();
    if action_tokens.is_empty() {
        return Err("missing action name".to_string());
    }
    let action_name = action_tokens.join(" ");

    Ok((keystroke.to_string(), is_global, action_name, ctx))
}

pub fn last_segment_after_unescaped_slash(s: &str) -> &str {
    let ci: Vec<(usize, char)> = s.char_indices().collect();
    // walk backward over the char-index pairs
    for i in (0..ci.len()).rev() {
        let (idx, ch) = ci[i];
        if ch != '/' {
            continue;
        }
        // if there's a char before this slash, check if it is a backslash
        if i == 0 {
            // slash at start -> nothing before it, so take everything after
            return &s[idx + ch.len_utf8()..];
        }
        let (_prev_idx, prev_ch) = ci[i - 1];
        if prev_ch == '\\' {
            // escaped slash -> skip it
            continue;
        }
        // found an unescaped '/'
        return &s[idx + ch.len_utf8()..];
    }
    // no unescaped slash found -> return full string
    s
}


#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_example_not_global() {
        let line = "\"Ctrl k\" 0 0 toggle_stop_after_album";
        let (keystroke, is_global, action, _) = parse_line(line).expect("parse failed");
        assert_eq!(keystroke, "Ctrl k");
        assert_eq!(is_global, false);
        assert_eq!(action, "toggle_stop_after_album");
    }

    #[test]
    fn parses_example_global_and_action_with_spaces() {
        let line = "\"Alt+X\" 123 1 do something now";
        let (keystroke, is_global, action, _) = parse_line(line).expect("parse failed");
        assert_eq!(keystroke, "Alt+X");
        assert_eq!(is_global, true);
        assert_eq!(action, "do something now");
    }

    #[test]
    fn errors_when_missing_quote() {
        let line = "Ctrl k\" 0 0 action";
        assert!(parse_line(line).is_err());
    }

    use super::last_segment_after_unescaped_slash;

    #[test]
    fn no_slash_returns_whole_string() {
        assert_eq!(last_segment_after_unescaped_slash("abc"), "abc");
        assert_eq!(last_segment_after_unescaped_slash(""), "");
    }

    #[test]
    fn simple_unescaped_slash() {
        assert_eq!(last_segment_after_unescaped_slash("a/b"), "b");
        assert_eq!(last_segment_after_unescaped_slash("hello/world"), "world");
    }

    #[test]
    fn trailing_slash_returns_empty() {
        assert_eq!(last_segment_after_unescaped_slash("abc/"), "");
    }

    #[test]
    fn only_slash_at_start() {
        assert_eq!(last_segment_after_unescaped_slash("/abc"), "abc");
        assert_eq!(last_segment_after_unescaped_slash("/"), "");
    }

    #[test]
    fn escaped_slash_is_ignored() {
        assert_eq!(last_segment_after_unescaped_slash("a\\/b/c"), "c");
        assert_eq!(last_segment_after_unescaped_slash("a\\/b"), "a\\/b");
    }

    #[test]
    fn multiple_escaped_and_unescaped_slashes() {
        assert_eq!(last_segment_after_unescaped_slash("x\\/y\\/z/fin"), "fin");
        assert_eq!(last_segment_after_unescaped_slash("one\\/two/three\\/four/five"), "five");
    }

    #[test]
    fn utf8_characters() {
        assert_eq!(last_segment_after_unescaped_slash("å/ø"), "ø");
        assert_eq!(last_segment_after_unescaped_slash("テスト/終わり"), "終わり");
    }

    #[test]
    fn escaped_slash_at_start() {
        assert_eq!(last_segment_after_unescaped_slash("\\/abc/def"), "def");
    }
}
