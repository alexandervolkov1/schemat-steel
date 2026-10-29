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

    fn assert_scheme_format(source: &str, expected: &str) {
        let formatted = format_string(source).unwrap();
        assert_eq!(formatted, expected, "source: {source}");
        assert_eq!(
            format_string(&formatted).unwrap(),
            formatted,
            "source: {source}"
        );
    }

    #[test]
    fn scheme_call_indentation() {
        for (source, expected) in [
            (
                "(compute first\n second\n (nested x\n y))",
                "(compute first\n         second\n         (nested x\n                 y))\n",
            ),
            (
                "(long-function\n first\n second)",
                "(long-function\n first\n second)\n",
            ),
            ("(f a\n b)", "(f a\n   b)\n"),
        ] {
            assert_scheme_format(source, expected);
        }
    }

    #[test]
    fn scheme_call_indentation_after_unicode() {
        assert_scheme_format("(λ first\n second)", "(λ first\n   second)\n");
        assert_scheme_format("(f \"λ\" (g a\n b))", "(f \"λ\" (g a\n          b))\n");
        assert_scheme_format("(ж first\n second)", "(ж first\n   second)\n");
        assert_scheme_format("(漢 first\n second)", "(漢 first\n    second)\n");
        assert_scheme_format("(e\u{301} first\n second)", "(e\u{301} first\n   second)\n");
        assert_scheme_format(
            "(f λ (g first\n second))",
            "(f λ (g first\n        second))\n",
        );
        assert_scheme_format(
            "(hash \"a\nb\" (g first\n second))",
            "(hash \"a\nb\" (g first\n      second))\n",
        );
    }

    #[test]
    fn scheme_definition_indentation() {
        for (source, expected) in [
            (
                "(define answer\n (compute x\n y))",
                "(define answer\n  (compute x\n           y))\n",
            ),
            (
                "(define (process x y)\n (display x)\n (combine x y))",
                "(define (process x y)\n  (display x)\n  (combine x y))\n",
            ),
            (
                "(begin (define (f x)\n (display x)\n (+ x 1))\n (f 1))",
                "(begin (define (f x)\n         (display x)\n         (+ x 1))\n       (f 1))\n",
            ),
            (
                "(begin\n (define x 1)\n x)",
                "(begin\n  (define x 1)\n  x)\n",
            ),
        ] {
            assert_scheme_format(source, expected);
        }
    }

    #[test]
    fn scheme_lambda_indentation() {
        for (source, expected) in [
            (
                "(map (lambda (x)\n (display x)\n (+ x 1))\n xs)",
                "(map (lambda (x)\n       (display x)\n       (+ x 1))\n     xs)\n",
            ),
            (
                "(lambda\n (x y)\n (display x)\n (+ x y))",
                "(lambda\n    (x y)\n  (display x)\n  (+ x y))\n",
            ),
            ("(lambda (x) (+ x 1))", "(lambda (x) (+ x 1))\n"),
        ] {
            assert_scheme_format(source, expected);
        }
    }

    #[test]
    fn scheme_binding_indentation() {
        for (source, expected) in [
            (
                "(f (let ((x 1)\n (y 2))\n (+ x y)))",
                "(f (let ((x 1)\n         (y 2))\n     (+ x y)))\n",
            ),
            (
                "(let\n ((x 1)\n (y 2))\n (+ x y))",
                "(let\n  ((x 1)\n   (y 2))\n  (+ x y))\n",
            ),
            (
                "(let*\n ((x 1)\n (y 2))\n (+ x y))",
                "(let*\n  ((x 1)\n   (y 2))\n  (+ x y))\n",
            ),
            (
                "(letrec\n ((x 1)\n (y 2))\n (+ x y))",
                "(letrec\n  ((x 1)\n   (y 2))\n  (+ x y))\n",
            ),
            (
                "(let loop\n ((n 3)\n (acc 0))\n (loop (- n 1) acc))",
                "(let loop\n  ((n 3)\n   (acc 0))\n  (loop (- n 1) acc))\n",
            ),
            (
                "(define (work x)\n (let* ((a (prepare x))\n (f (lambda (v)\n (display v)\n (finish v))))\n (f a)))",
                "(define (work x)\n  (let* ((a (prepare x))\n         (f (lambda (v)\n              (display v)\n              (finish v))))\n    (f a)))\n",
            ),
        ] {
            assert_scheme_format(source, expected);
        }
    }

    #[test]
    fn scheme_comments_and_blank_lines() {
        for (source, expected) in [
            (
                "(f first ; first\n ; next\n second\n\n third)",
                "(f first ; first\n   ; next\n   second\n\n   third)\n",
            ),
            (
                "(let ((x 1)\n ; binding\n (y 2))\n #| body |#\n (+ x y))",
                "(let ((x 1)\n      ; binding\n      (y 2))\n  #|\n  body\n  |#\n  (+ x y))\n",
            ),
        ] {
            assert_scheme_format(source, expected);
        }
    }

    #[test]
    fn format_if_branches() {
        for (source, expected) in [
            ("(if condition yes no)", "(if condition yes no)\n"),
            (
                "(if (> remaining 1)\n (time/sleep-ms pause-ms)\n #t)",
                "(if (> remaining 1)\n    (time/sleep-ms pause-ms)\n    #t)\n",
            ),
            (
                "(if ready?\n (if (> x 0)\n (start)\n (stop))\n (wait))",
                "(if ready?\n    (if (> x 0)\n        (start)\n        (stop))\n    (wait))\n",
            ),
            (
                "(if (and ready?\n valid?)\n (start)\n (stop))",
                "(if (and ready?\n         valid?)\n    (start)\n    (stop))\n",
            ),
            (
                "(if\n condition\n yes\n no)",
                "(if\n  condition\n    yes\n    no)\n",
            ),
            (
                "(if ready? ; condition\n ; then\n (start)\n\n ; else\n (stop))",
                "(if ready? ; condition\n    ; then\n    (start)\n\n    ; else\n    (stop))\n",
            ),
            (
                "'(if condition\n yes\n no)",
                "'(if condition\n  yes\n  no)\n",
            ),
            (
                "(foo (if condition\n yes\n no))",
                "(foo (if condition\n         yes\n         no))\n",
            ),
            (
                "(if test\n #| choose |# yes\n no)",
                "(if test\n    #| choose |# yes\n    no)\n",
            ),
            (
                "(if test\n yes ; done\n no)",
                "(if test\n    yes ; done\n    no)\n",
            ),
            (
                "(if ; condition follows\n test\n yes\n no)",
                "(if ; condition follows\n  test\n    yes\n    no)\n",
            ),
            (
                "(if test\n yes\n no ; final\n)",
                "(if test\n    yes\n    no) ; final\n",
            ),
            ("(if test\n\n yes\n no)", "(if test\n\n    yes\n    no)\n"),
            (
                "(quasiquote (if condition\n yes\n no))",
                "(quasiquote (if condition\n             yes\n             no))\n",
            ),
            (
                "(quasiquote (unquote (if condition\n yes\n no)))",
                "(quasiquote (unquote (if condition\n                         yes\n                         no)))\n",
            ),
        ] {
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
                "(hash a (f x\n           y)\n      b c)\n",
            ),
            ("(hash a\n b\n c)", "(hash a\n      b\n      c)\n"),
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
    fn hash_in_named_quasiquote_and_unquote() {
        for (source, expected) in [
            (
                "(quasiquote (hash a\n b\n c\n d))",
                "(quasiquote (hash a\n             b\n             c\n             d))\n",
            ),
            (
                "(quasiquote (unquote (hash a\n b\n c\n d)))",
                "(quasiquote (unquote (hash a b\n                           c d)))\n",
            ),
            (
                "(quote (unquote (hash a\n b\n c\n d)))",
                "(quote (unquote (hash a\n                 b\n                 c\n                 d)))\n",
            ),
        ] {
            let formatted = format_string(source).unwrap();
            assert_eq!(formatted, expected, "source: {source}");
            assert_eq!(format_string(&formatted).unwrap(), formatted);
        }
    }

    #[test]
    fn hash_in_quasiquote_inside_quote_remains_data() {
        let formatted =
            format_string("(quote (quasiquote (unquote (hash a\n b\n c\n d))))").unwrap();
        assert!(!formatted.contains("a b"), "{formatted}");
        assert!(!formatted.contains("c d"), "{formatted}");
        assert_eq!(format_string(&formatted).unwrap(), formatted);
    }

    #[test]
    fn hash_in_nested_quasiquote_with_one_unquote_remains_data() {
        let formatted =
            format_string("(quasiquote (quasiquote (unquote (hash a\n b\n c\n d))))").unwrap();
        assert!(!formatted.contains("a b"), "{formatted}");
        assert!(!formatted.contains("c d"), "{formatted}");
        assert_eq!(format_string(&formatted).unwrap(), formatted);
    }

    #[test]
    fn hash_in_nested_quasiquote_with_two_unquotes_is_code() {
        for source in [
            "(quasiquote (quasiquote (unquote (unquote (hash a\n b\n c\n d)))))",
            "``(unquote ,(hash a\n b\n c\n d))",
        ] {
            let formatted = format_string(source).unwrap();
            assert!(formatted.contains("a b"), "{formatted}");
            assert!(formatted.contains("c d"), "{formatted}");
            assert_eq!(format_string(&formatted).unwrap(), formatted);
        }
    }

    #[test]
    fn hash_in_mixed_nested_quotes_remains_data() {
        for source in [
            "'`(unquote (hash a\n b\n c\n d))",
            "`(quasiquote ,(hash a\n b\n c\n d))",
            "(quasiquote `(unquote (hash a\n b\n c\n d)))",
        ] {
            let formatted = format_string(source).unwrap();
            assert!(!formatted.contains("a b"), "{formatted}");
            assert!(!formatted.contains("c d"), "{formatted}");
            assert_eq!(format_string(&formatted).unwrap(), formatted);
        }
    }

    #[test]
    fn hash_preserves_blank_line_between_key_and_value() {
        assert_eq!(
            format_string("(other a\n\n b)").unwrap(),
            "(other a\n\n       b)\n"
        );
        for (source, expected) in [
            ("(hash a\n\n b\n c d)", "(hash a\n\n      b\n      c d)\n"),
            ("(hash a\n\n\n b\n c d)", "(hash a\n\n      b\n      c d)\n"),
            (
                "(hash a\n\n ; value\n b\n c d)",
                "(hash a\n\n      ; value\n      b\n      c d)\n",
            ),
            (
                "(hash a\n ; value\n\n b\n c d)",
                "(hash a\n      ; value\n\n      b\n      c d)\n",
            ),
        ] {
            assert_eq!(format_string(source).unwrap(), expected, "source: {source}");
            assert_eq!(format_string(expected).unwrap(), expected);
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
