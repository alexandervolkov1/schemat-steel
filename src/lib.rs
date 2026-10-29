#![doc = include_str!("../README.md")]

extern crate alloc;

mod ast;
mod context;
mod error;
mod file;
mod format;
mod parse;
mod position;
mod position_map;

pub use self::{
    error::Error,
    file::{display_path, read_paths},
};
use self::{
    format::format,
    parse::{ParseError, parse, parse_comments, parse_hash_directives},
    position_map::PositionMap,
};
use bumpalo::Bump;

/// Formats a source string.
pub fn format_string(source: &str) -> Result<String, Error> {
    let position_map = PositionMap::new(source);
    let convert_error = |error| convert_parse_error(error, source, &position_map);
    let allocator = Bump::new();

    Ok(format(
        &parse(source).map_err(convert_error)?,
        &parse_comments(source).map_err(convert_error)?,
        &parse_hash_directives(source).map_err(convert_error)?,
        &position_map,
        &allocator,
    )?)
}

fn convert_parse_error(error: ParseError, source: &str, position_map: &PositionMap) -> Error {
    Error::Parse(error.to_string(source, position_map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn format_empty_string() {
        assert_eq!(format_string("").unwrap(), "\n");
    }

    #[test]
    fn format_list() {
        assert_eq!(format_string("( foo  bar )").unwrap(), "(foo bar)\n");
    }

    #[test]
    fn format_hash_pairs() {
        let cases = [
            ("(hash \"id\" id)", "(hash \"id\" id)\n"),
            (
                "(hash \"id\" id\n \"op\"\n \"set\"\n \"key\"\n key\n \"value\"\n value)",
                "(hash \"id\" id\n      \"op\" \"set\"\n      \"key\" key\n      \"value\" value)\n",
            ),
            (
                "(hash \"a\"\n (f x)\n \"b\"\n (hash \"c\"\n 1\n \"d\"\n 2))",
                "(hash \"a\" (f x)\n      \"b\" (hash \"c\" 1\n                \"d\" 2))\n",
            ),
            (
                "(hash \"a\" 1\n ; before key\n \"b\" #| inline |# 2)",
                "(hash \"a\" 1\n      ; before key\n      \"b\" #| inline |# 2)\n",
            ),
            (
                "(hash \"a\" ; value follows\n 1\n \"b\" 2)",
                "(hash \"a\" ; value follows\n      1\n      \"b\" 2)\n",
            ),
            (
                "(hash a\n ; value\n b\n c d)",
                "(hash a\n      ; value\n      b\n      c d)\n",
            ),
            (
                "(hash a\n #| value |#\n b\n c d)",
                "(hash a\n      #|\n      value\n      |#\n      b\n      c d)\n",
            ),
            ("(hash ; head\n a b\n c d)", "(hash ; head\n  a b\n  c d)\n"),
            (
                "(hash a\n (f x\n y)\n b c)",
                "(hash a (f x\n         y)\n      b c)\n",
            ),
            ("(hash a\n b\n c)", "(hash a\n  b\n  c)\n"),
            ("'(hash a\n b\n c\n d)", "'(hash a\n  b\n  c\n  d)\n"),
            (
                "(quote (hash a\n b\n c\n d))",
                "(quote (hash a\n        b\n        c\n        d))\n",
            ),
        ];

        for (source, expected) in cases {
            let formatted = format_string(source).unwrap();
            assert_eq!(formatted, expected, "source: {source}");
            assert_eq!(
                format_string(&formatted).unwrap(),
                formatted,
                "source: {source}"
            );
        }
    }

    #[test]
    fn fail_to_format_unclosed_list() {
        assert!(matches!(format_string("(foo"), Err(Error::Parse(_))));
    }

    #[test]
    fn fail_to_format_unclosed_list_after_multi_byte_characters() {
        assert_eq!(
            format_string("λλ (foo").unwrap_err().to_string(),
            "failed to parse at line 1 and column 7: λλ (foo"
        );
    }

    #[test]
    fn fail_to_format_unclosed_list_ending_with_multi_byte_character() {
        assert_eq!(
            format_string("(λ").unwrap_err().to_string(),
            "failed to parse at line 1 and column 2: (λ"
        );
    }
}
