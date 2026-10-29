#![expect(unstable_name_collisions)]

use crate::{
    ast::{Comment, Expression, HashDirective},
    context::Context,
    position::Position,
    position_map::PositionMap,
};
use allocator_api2::{alloc::Allocator, vec::Vec};
use core::fmt;
use itertools::Itertools;
use mfmt::{Builder, Document, FormatOptions, empty, line, sequence, utility::is_empty};

const COMMENT_PREFIX: &str = ";";
const QUOTE_SIGNS: &[&str] = &["'", "`", "#"];
const UNQUOTE_SIGNS: &[&str] = &[",", ",@"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quoted {
    No,
    Quote,
    Quasiquote(usize),
}

impl Quoted {
    const fn quasiquote(self) -> Self {
        match self {
            Self::No => Self::Quasiquote(1),
            Self::Quote => Self::Quote,
            Self::Quasiquote(depth) => Self::Quasiquote(depth + 1),
        }
    }

    const fn unquote(self) -> Self {
        match self {
            Self::Quasiquote(1) => Self::No,
            Self::Quasiquote(depth) => Self::Quasiquote(depth - 1),
            _ => self,
        }
    }
}

pub fn format<A: Allocator + Clone>(
    module: &[Expression],
    comments: &[Comment],
    hash_directives: &[HashDirective],
    position_map: &PositionMap,
    allocator: A,
) -> Result<String, fmt::Error> {
    let mut string = Default::default();
    let document = compile_module(
        &mut Context::new(Builder::new(allocator), comments, position_map),
        module,
        hash_directives,
    );

    mfmt::format(
        &if is_empty(&document) {
            line()
        } else {
            document
        },
        &mut string,
        FormatOptions::new(2),
    )?;

    Ok(string)
}

fn compile_module<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    module: &'a [Expression<'a>],
    hash_directives: &[HashDirective],
) -> Document<'a> {
    [
        if hash_directives.is_empty() {
            empty()
        } else {
            context.builder().sequence([context.builder().sequence(
                hash_directives
                    .iter()
                    .map(|directive| compile_hash_directive(context, directive)),
            )])
        },
        {
            let expressions = compile_expressions(context, module, false, Quoted::No);

            if is_empty(&expressions) {
                empty()
            } else {
                context.builder().sequence([expressions, line()])
            }
        },
        compile_remaining_block_comment(context),
    ]
    .into_iter()
    .fold(empty(), |all, document| {
        if is_empty(&document) {
            all
        } else {
            context.builder().sequence([
                if is_empty(&all) {
                    empty()
                } else {
                    context.builder().sequence([all, line()])
                },
                document,
            ])
        }
    })
}

fn compile_hash_directive<'a, A: Allocator + Clone + 'a>(
    context: &Context<A>,
    hash_directive: &HashDirective,
) -> Document<'a> {
    context.builder().sequence([
        context.builder().strings(["#", hash_directive.value()]),
        line(),
    ])
}

fn compile_expression<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expression: &'a Expression<'a>,
    data: bool,
    quoted: Quoted,
) -> Document<'a> {
    compile_comment(context, expression.position(), |context| match expression {
        Expression::List(left, right, expressions, position) => {
            compile_list(context, expressions, position, left, right, data, quoted)
        }
        Expression::Quote(sign, expression, _) => context.builder().clone().sequence([
            (*sign).into(),
            compile_expression(
                context,
                expression,
                QUOTE_SIGNS.contains(sign) || !UNQUOTE_SIGNS.contains(sign) && data,
                match *sign {
                    "`" => quoted.quasiquote(),
                    "'" | "#" | "#;" => Quoted::Quote,
                    "," | ",@" => quoted.unquote(),
                    _ => quoted,
                },
            ),
        ]),
        Expression::QuotedSymbol(symbol, _) => context.builder().sequence(["|", *symbol, "|"]),
        Expression::String(string, _) => context.builder().sequence(["\"", *string, "\""]),
        Expression::Symbol(name, _) => (*name).into(),
    })
}

fn compile_list<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    position: &Position,
    left: &'a str,
    right: &'a str,
    data: bool,
    quoted: Quoted,
) -> Document<'a> {
    let hash = !data
        && quoted == Quoted::No
        && left == "("
        && matches!(expressions.first(), Some(Expression::Symbol("hash", _)))
        && expressions.len() > 1
        && (expressions.len() - 1).is_multiple_of(2);
    let if_form = !data
        && quoted == Quoted::No
        && left == "("
        && matches!(expressions.first(), Some(Expression::Symbol("if", _)))
        && (3..=4).contains(&expressions.len());
    let code = !data
        && quoted == Quoted::No
        && !hash
        && !if_form
        && !matches!(
            expressions.first(),
            Some(Expression::Symbol("quote" | "quasiquote" | "unquote", _))
        );
    let body_form = code
        && (matches!(
            expressions.first(),
            Some(Expression::Symbol(
                "define" | "lambda" | "let" | "let*" | "letrec",
                _
            ))
        ) || matches!(expressions.first(), Some(Expression::Symbol("begin", _)))
            && expressions.get(1).is_some_and(|expression| {
                line_index(context, expression.position().start())
                    > line_index(context, position.start())
            }));
    let lambda = code && matches!(expressions.first(), Some(Expression::Symbol("lambda", _)));
    let children_quoted = match expressions.first() {
        Some(Expression::Symbol("quote", _)) => Quoted::Quote,
        Some(Expression::Symbol("quasiquote", _)) => quoted.quasiquote(),
        Some(Expression::Symbol("unquote", _)) => quoted.unquote(),
        _ => quoted,
    };
    let children_data = if matches!(quoted, Quoted::Quasiquote(_))
        && matches!(expressions.first(), Some(Expression::Symbol("unquote", _)))
    {
        false
    } else {
        data
    };
    let index = line_index(context, position.start());

    let index = expressions
        .iter()
        .position(|expression| line_index(context, expression.position().start()) > index)
        .unwrap_or(expressions.len());
    let first = &expressions[..index];
    let last = &expressions[index..];
    let call = code
        && !body_form
        && first.len() >= 2
        && matches!(expressions.first(), Some(Expression::Symbol(_, _)));

    let builder = context.builder().clone();
    let hash_breaks = if hash {
        expressions
            .windows(2)
            .map(|pair| has_intervening_line_comment(context, &pair[0], &pair[1]))
            .collect::<alloc::vec::Vec<_>>()
    } else {
        alloc::vec::Vec::new()
    };

    let document = builder.sequence([
        compile_comment(
            context,
            &position.set_end(position.start() + left.len()),
            |_| left.into(),
        ),
        builder.indent(
            builder.offside(
                builder.sequence(
                    [if hash {
                        compile_hash_expressions(context, expressions, position, &hash_breaks)
                    } else if call {
                        compile_call_expressions(
                            context,
                            expressions,
                            index,
                            children_data,
                            children_quoted,
                        )
                    } else {
                        builder.flatten(compile_expressions(
                            context,
                            first,
                            children_data,
                            children_quoted,
                        ))
                    }]
                    .into_iter()
                    .chain(match (hash, first.last(), last.first()) {
                        (false, Some(first), Some(last))
                            if !if_form
                                && !call
                                && line_gap(context, first.position(), last.position()) > 1 =>
                        {
                            Some(line())
                        }
                        _ => None,
                    })
                    .chain(if hash || last.is_empty() || call {
                        None
                    } else if if_form {
                        Some(compile_if_remaining(context, expressions, index))
                    } else if lambda && index <= 1 {
                        Some(compile_lambda_remaining(context, expressions, index))
                    } else {
                        Some(builder.r#break(builder.sequence([
                            line(),
                            compile_expressions(context, last, children_data, children_quoted),
                        ])))
                    })
                    .chain({
                        let right_position = position.set_start(position.end() - right.len());
                        let gap = line_gap(
                            context,
                            expressions
                                .last()
                                .map(Expression::position)
                                .unwrap_or(position),
                            &right_position,
                        ) > 1;
                        let block_comment = compile_block_comment(context, &right_position);
                        let inline_comment = compile_inline_comment(context, &right_position);
                        let block_comment_empty = is_empty(&block_comment);

                        [builder.r#break(builder.sequence([
                            if block_comment_empty {
                                empty()
                            } else {
                                builder.sequence([if gap { line() } else { empty() }, line()])
                            },
                            block_comment,
                            if block_comment_empty
                                && !is_empty(&inline_comment)
                                && !expressions.is_empty()
                            {
                                " ".into()
                            } else {
                                empty()
                            },
                            inline_comment,
                            right.into(),
                        ]))]
                    }),
                ),
                !data && !(code && !body_form),
            ),
        ),
    ]);
    if if_form || body_form {
        builder.offside(document, false)
    } else {
        document
    }
}

fn compile_call_expressions<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    index: usize,
    data: bool,
    quoted: Quoted,
) -> Document<'a> {
    let builder = context.builder().clone();
    let head = compile_expression(context, &expressions[0], data, quoted);
    let first = compile_expressions(context, &expressions[1..index], data, quoted);
    let gap = index < expressions.len()
        && line_gap(
            context,
            expressions[index - 1].position(),
            expressions[index].position(),
        ) > 1;
    let last = compile_expressions(context, &expressions[index..], data, quoted);
    builder.sequence([
        head,
        builder.flatten(line()),
        builder.offside(
            builder.sequence([
                builder.flatten(first),
                if gap {
                    builder.r#break(line())
                } else {
                    empty()
                },
                if index < expressions.len() {
                    builder.r#break(builder.sequence([line(), last]))
                } else {
                    empty()
                },
            ]),
            false,
        ),
    ])
}

fn compile_lambda_remaining<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    start: usize,
) -> Document<'a> {
    let builder = context.builder().clone();
    let mut documents =
        Vec::with_capacity_in(expressions.len() - start, builder.allocator().clone());
    for (index, expression) in expressions.iter().enumerate().skip(start) {
        let gap = index > 0
            && line_gap(
                context,
                expressions[index - 1].position(),
                expression.position(),
            ) > 1;
        let document = builder.sequence([
            line(),
            if gap { line() } else { empty() },
            compile_expression(context, expression, false, Quoted::No),
        ]);
        documents.push(if index == 1 {
            builder.indent(document)
        } else {
            document
        });
    }
    builder.r#break(sequence(documents.leak()))
}

fn compile_if_remaining<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    start: usize,
) -> Document<'a> {
    let builder = context.builder().clone();
    let mut documents =
        Vec::with_capacity_in(expressions.len() - start, builder.allocator().clone());
    for (index, expression) in expressions.iter().enumerate().skip(start) {
        let gap = index > 0
            && line_gap(
                context,
                expressions[index - 1].position(),
                expression.position(),
            ) > 1;
        let document = builder.sequence([
            line(),
            if gap { line() } else { empty() },
            compile_expression(context, expression, false, Quoted::No),
        ]);
        documents.push(if index >= 2 {
            builder.indent(document)
        } else {
            document
        });
    }
    builder.r#break(sequence(documents.leak()))
}

fn compile_hash_expressions<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    position: &Position,
    breaks: &[bool],
) -> Document<'a> {
    let builder = context.builder().clone();
    let multiline = expressions.iter().any(|expression| {
        line_index(context, expression.position().start()) > line_index(context, position.start())
    });
    let mut documents = Vec::with_capacity_in(expressions.len() * 2, builder.allocator().clone());
    let head = &expressions[0];
    let head_document = compile_expression(context, head, false, Quoted::No);
    for (index, pair) in expressions[1..].as_chunks::<2>().0.iter().enumerate() {
        if index > 0 {
            documents.push(if multiline {
                builder.r#break(line())
            } else {
                builder.flatten(line())
            });
            if line_gap(
                context,
                expressions[index * 2].position(),
                pair[0].position(),
            ) > 1
            {
                documents.push(builder.r#break(line()));
            }
        }
        let blank_line = line_gap(context, pair[0].position(), pair[1].position()) > 1;
        documents.push(compile_expression(context, &pair[0], false, Quoted::No));
        documents.push(if breaks[index * 2 + 1] || blank_line {
            builder.r#break(line())
        } else {
            builder.flatten(line())
        });
        if blank_line {
            documents.push(builder.r#break(line()));
        }
        documents.push(compile_expression(context, &pair[1], false, Quoted::No));
    }
    builder.sequence([
        head_document,
        if breaks[0] {
            builder.r#break(line())
        } else {
            builder.flatten(line())
        },
        builder.offside(sequence(documents.leak()), false),
    ])
}

fn has_intervening_line_comment<A: Allocator + Clone>(
    context: &Context<A>,
    previous: &Expression,
    next: &Expression,
) -> bool {
    context
        .peek_comments(line_index(context, next.position().start()) + 1)
        .any(|comment| {
            comment.position().start() >= previous.position().end()
                && comment.position().end() <= next.position().start()
                && (matches!(comment, Comment::Line(_))
                    || line_index(context, comment.position().start())
                        > line_index(context, previous.position().end() - 1)
                    || line_index(context, comment.position().start())
                        != line_index(context, comment.position().end() - 1))
        })
}

fn compile_expressions<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    expressions: &'a [Expression<'a>],
    data: bool,
    quoted: Quoted,
) -> Document<'a> {
    let mut documents =
        Vec::with_capacity_in(2 * expressions.len(), context.builder().allocator().clone());
    let mut last_expression = None::<&Expression>;

    for expression in expressions {
        if let Some(last_expression) = last_expression {
            documents.push(line());

            if line_gap(context, last_expression.position(), expression.position()) > 1 {
                documents.push(line());
            }
        }

        documents.push(compile_expression(context, expression, data, quoted));

        last_expression = Some(expression);
    }

    sequence(documents.leak())
}

fn compile_comment<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    position: &Position,
    document: impl Fn(&mut Context<'a, A>) -> Document<'a>,
) -> Document<'a> {
    let block_comment = compile_block_comment(context, position);
    let inline_comment = compile_inline_comment(context, position);
    let inline_space = if is_empty(&inline_comment) {
        empty()
    } else {
        " ".into()
    };
    let document = document(context);
    let suffix_comment = compile_suffix_comment(context, position);

    context.builder().sequence([
        block_comment,
        inline_comment,
        inline_space,
        document,
        suffix_comment,
    ])
}

fn compile_inline_comment<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    position: &Position,
) -> Document<'a> {
    let builder = context.builder().clone();

    builder.sequence(
        context
            .drain_inline_comments(position)
            .map(|comment| builder.sequence(["#|", comment.content(), "|#"]))
            .intersperse(" ".into()),
    )
}

fn compile_suffix_comment<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<A>,
    position: &Position,
) -> Document<'a> {
    let builder = context.builder().clone();

    builder.sequence(
        context
            .drain_current_line_comment(line_index(context, position.end() - 1))
            .map(|comment| {
                builder.line_suffixes([" ", COMMENT_PREFIX, comment.content().trim_end()])
            }),
    )
}

fn compile_block_comment<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
    position: &Position,
) -> Document<'a> {
    let builder = context.builder().clone();
    let comments = builder
        .allocate_slice(context.drain_multi_line_comments(line_index(context, position.start())));

    compile_all_comments(context, comments, Some(position))
}

fn compile_remaining_block_comment<'a, A: Allocator + Clone + 'a>(
    context: &mut Context<'a, A>,
) -> Document<'a> {
    let builder = context.builder().clone();
    let comments = builder.allocate_slice(context.drain_multi_line_comments(usize::MAX));

    compile_all_comments(context, comments, None)
}

fn compile_all_comments<'a, A: Allocator + Clone + 'a>(
    context: &Context<A>,
    comments: &'a [&'a Comment<'a>],
    last_position: Option<&Position>,
) -> Document<'a> {
    context.builder().sequence(
        comments
            .iter()
            .zip(
                comments
                    .iter()
                    .skip(1)
                    .map(|comment| comment.position())
                    .chain([last_position.unwrap_or(&Position::new(0, 0))]),
            )
            .map(|(comment, next_position)| {
                let builder = context.builder();

                builder.sequence([
                    match comment {
                        Comment::Block(comment) => builder.sequence([
                            "#|".into(),
                            line(),
                            comment.content().trim().into(),
                            line(),
                            "|#".into(),
                            line(),
                        ]),
                        Comment::Line(comment) => builder.sequence([
                            COMMENT_PREFIX.into(),
                            comment.content().trim_end().into(),
                            context.builder().r#break(line()),
                        ]),
                    },
                    if line_gap(context, comment.position(), next_position) > 1 {
                        line()
                    } else {
                        empty()
                    },
                ])
            }),
    )
}

fn line_gap<A: Allocator + Clone>(
    context: &Context<A>,
    previous_position: &Position,
    position: &Position,
) -> usize {
    let index = line_index(context, position.start());

    context
        .peek_comments(index)
        .next()
        .map(|comment| line_index(context, comment.position().start()))
        .unwrap_or(index)
        .saturating_sub(line_index(context, previous_position.end() - 1))
}

fn line_index<A: Allocator + Clone>(context: &Context<A>, offset: usize) -> usize {
    context
        .position_map()
        .line_index(offset)
        .expect("valid offset")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::{BlockComment, LineComment},
        position::Position,
        position_map::PositionMap,
    };
    use allocator_api2::alloc::Global;
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    fn format(
        module: &[Expression],
        comments: &[Comment],
        hash_directives: &[HashDirective],
        source: &str,
    ) -> Result<String, fmt::Error> {
        super::format(
            module,
            comments,
            hash_directives,
            &PositionMap::new(source),
            Global,
        )
    }

    #[test]
    fn format_empty() {
        assert_eq!(format(&[], &[], &[], "\n").unwrap(), "\n");
    }

    #[test]
    fn format_list() {
        assert_eq!(
            format(
                &[Expression::List(
                    "(",
                    ")",
                    vec![
                        Expression::Symbol("foo", Position::new(0, 2)),
                        Expression::Symbol("bar", Position::new(0, 2))
                    ],
                    Position::new(0, 2)
                )],
                &[],
                &[],
                "(foo bar)",
            )
            .unwrap(),
            "(foo bar)\n"
        );
    }

    #[test]
    fn format_list_with_split_lines() {
        assert_eq!(
            format(
                &[Expression::List(
                    "(",
                    ")",
                    vec![
                        Expression::Symbol("foo", Position::new(1, 4)),
                        Expression::Symbol("bar", Position::new(5, 8))
                    ],
                    Position::new(0, 9)
                )],
                &[],
                &[],
                "(foo\nbar)",
            )
            .unwrap(),
            indoc!(
                "
                (foo
                 bar)
                "
            )
        );
    }

    #[test]
    fn format_list_with_split_lines_and_multiple_elements() {
        assert_eq!(
            format(
                &[Expression::List(
                    "(",
                    ")",
                    vec![
                        Expression::Symbol("foo", Position::new(0, 1)),
                        Expression::Symbol("bar", Position::new(0, 1)),
                        Expression::Symbol("baz", Position::new(2, 3)),
                        Expression::Symbol("qux", Position::new(2, 3)),
                    ],
                    Position::new(0, 1)
                )],
                &[],
                &[],
                "a\nb",
            )
            .unwrap(),
            indoc!(
                "
                (foo bar
                     baz
                     qux)
                "
            )
        );
    }

    #[test]
    fn format_quote() {
        assert_eq!(
            format(
                &[Expression::Quote(
                    "'",
                    Expression::Symbol("foo", Position::new(0, 3)).into(),
                    Position::new(0, 3)
                )],
                &[],
                &[],
                "'foo",
            )
            .unwrap(),
            "'foo\n"
        );
    }

    #[test]
    fn format_unquote() {
        assert_eq!(
            format(
                &[Expression::Quote(
                    ",",
                    Expression::Symbol("foo", Position::new(0, 3)).into(),
                    Position::new(0, 3)
                )],
                &[],
                &[],
                "'foo",
            )
            .unwrap(),
            ",foo\n"
        );
    }

    #[test]
    fn format_string() {
        assert_eq!(
            format(
                &[Expression::String("foo", Position::new(0, 3))],
                &[],
                &[],
                "\"foo\"",
            )
            .unwrap(),
            "\"foo\"\n"
        );
    }

    #[test]
    fn format_multi_line_string() {
        assert_eq!(
            format(
                &[Expression::String("a\\\nb", Position::new(0, 6))],
                &[],
                &[],
                "\"a\\\nb\"",
            )
            .unwrap(),
            "\"a\\\nb\"\n"
        );
    }

    #[test]
    fn format_symbol() {
        assert_eq!(
            format(
                &[Expression::Symbol("foo", Position::new(0, 3))],
                &[],
                &[],
                "foo",
            )
            .unwrap(),
            "foo\n"
        );
    }

    #[test]
    fn format_quoted_symbol() {
        assert_eq!(
            format(
                &[Expression::QuotedSymbol("foo", Position::new(0, 3))],
                &[],
                &[],
                "foo",
            )
            .unwrap(),
            "|foo|\n"
        );
    }

    #[test]
    fn format_vector() {
        assert_eq!(
            format(
                &[Expression::List(
                    "[",
                    "]",
                    vec![
                        Expression::Symbol("foo", Position::new(0, 2)),
                        Expression::Symbol("bar", Position::new(0, 2))
                    ],
                    Position::new(0, 2)
                )],
                &[],
                &[],
                "[foo bar]",
            )
            .unwrap(),
            "[foo bar]\n"
        );
    }

    mod list {
        use super::*;
        use pretty_assertions::assert_eq;

        #[test]
        fn double_indent_of_nested_list() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(1, 2))
                            ],
                            Position::new(0, 1)
                        )],
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "\n\n\na",
                )
                .unwrap(),
                indoc!(
                    "
                    ((foo
                      bar))
                    "
                )
            );
        }

        #[test]
        fn format_blank_line() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::Symbol("bar", Position::new(1, 2))
                        ],
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "\na",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo
                     bar)
                    "
                )
            );
        }

        #[test]
        fn format_blank_lines() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(1, 4)),
                            Expression::Symbol("bar", Position::new(6, 9))
                        ],
                        Position::new(0, 10)
                    )],
                    &[],
                    &[],
                    "(foo\n\nbar)",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo

                     bar)
                    "
                )
            );
        }

        #[test]
        fn format_blank_lines_with_multi_line_expression() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("foo", Position::new(2, 5)),
                                    Expression::Symbol("bar", Position::new(6, 9))
                                ],
                                Position::new(1, 10)
                            ),
                            Expression::Symbol("baz", Position::new(12, 15))
                        ],
                        Position::new(0, 16)
                    )],
                    &[],
                    &[],
                    "((foo\nbar)\n\nbaz)",
                )
                .unwrap(),
                indoc!(
                    "
                    ((foo
                      bar)

                     baz)
                    "
                )
            );
        }

        #[test]
        fn format_first_element_on_second_line() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![Expression::Symbol("foo", Position::new(2, 5))],
                        Position::new(0, 6)
                    )],
                    &[],
                    &[],
                    "(\nfoo)",
                )
                .unwrap(),
                indoc!(
                    "
                    (
                     foo)
                    "
                )
            );
        }

        #[test]
        fn format_broken_nested_lists() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(6, 7))
                            ],
                            Position::new(0, 1)
                        ),],
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "((foo\nbar))",
                )
                .unwrap(),
                indoc!(
                    "
                    ((foo
                      bar))
                    "
                )
            );
        }

        #[test]
        fn format_broken_nested_lists_with_offside() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("bar", Position::new(0, 1)),
                                    Expression::Symbol("baz", Position::new(10, 11))
                                ],
                                Position::new(0, 1)
                            ),
                        ],
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "((foo bar\nbaz))",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo (bar
                          baz))
                    "
                )
            );
        }
    }

    mod module {
        use super::*;
        use pretty_assertions::assert_eq;

        #[test]
        fn keep_no_blank_line() {
            assert_eq!(
                format(
                    &[
                        Expression::Symbol("foo", Position::new(0, 1)),
                        Expression::Symbol("bar", Position::new(1, 2))
                    ],
                    &[],
                    &[],
                    "\na",
                )
                .unwrap(),
                indoc!(
                    "
                    foo
                    bar
                    "
                )
            );
        }

        #[test]
        fn keep_blank_line() {
            assert_eq!(
                format(
                    &[
                        Expression::Symbol("foo", Position::new(0, 1)),
                        Expression::Symbol("bar", Position::new(2, 3))
                    ],
                    &[],
                    &[],
                    "\n\na",
                )
                .unwrap(),
                indoc!(
                    "
                    foo

                    bar
                    "
                )
            );
        }
    }

    mod comment {
        use super::*;
        use pretty_assertions::assert_eq;

        #[test]
        fn format_multi_line_comment() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(1, 2))],
                    &[LineComment::new("bar", Position::new(0, 1)).into()],
                    &[],
                    "\na",
                )
                .unwrap(),
                indoc!(
                    "
                    ;bar
                    foo
                    "
                )
            );
        }

        #[test]
        fn format_multi_line_comment_with_extra_line() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(2, 3))],
                    &[LineComment::new("bar", Position::new(0, 1)).into()],
                    &[],
                    "\n\na",
                )
                .unwrap(),
                indoc!(
                    "
                    ;bar

                    foo
                    "
                )
            );
        }

        #[test]
        fn format_multi_line_comments() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(3, 4))],
                    &[
                        LineComment::new("bar", Position::new(0, 1)).into(),
                        LineComment::new("baz", Position::new(1, 2)).into()
                    ],
                    &[],
                    "\n\n\na",
                )
                .unwrap(),
                indoc!(
                    "
                    ;bar
                    ;baz

                    foo
                    "
                )
            );
        }

        #[test]
        fn format_multi_line_comments_with_extra_line() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(4, 5))],
                    &[
                        LineComment::new("bar", Position::new(0, 1)).into(),
                        LineComment::new("baz", Position::new(2, 3)).into()
                    ],
                    &[],
                    "\n\n\n\na",
                )
                .unwrap(),
                indoc!(
                    "
                    ;bar

                    ;baz

                    foo
                    "
                )
            );
        }

        #[test]
        fn format_multi_line_comment_after_expression() {
            assert_eq!(
                format(
                    &[
                        Expression::Symbol("foo", Position::new(0, 1)),
                        Expression::Symbol("baz", Position::new(2, 3))
                    ],
                    &[LineComment::new("bar", Position::new(1, 2)).into()],
                    &[],
                    "\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    foo
                    ;bar
                    baz
                    "
                )
            );
        }

        #[test]
        fn format_multi_line_comment_after_expression_in_list() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::Symbol("baz", Position::new(2, 3))
                        ],
                        Position::new(0, 1)
                    )],
                    &[LineComment::new("bar", Position::new(1, 2)).into()],
                    &[],
                    "\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo
                     ;bar
                     baz)
                    "
                )
            );
        }

        #[test]
        fn format_line_comment() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(0, 1))],
                    &[LineComment::new("bar", Position::new(0, 1)).into()],
                    &[],
                    "\na",
                )
                .unwrap(),
                indoc!(
                    "
                    foo ;bar
                    "
                )
            );
        }

        #[test]
        fn format_line_comments() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(0, 1))],
                    &[
                        LineComment::new("bar", Position::new(0, 1)).into(),
                        LineComment::new("baz", Position::new(0, 1)).into()
                    ],
                    &[],
                    "\na",
                )
                .unwrap(),
                indoc!(
                    "
                    foo ;bar ;baz
                    "
                )
            );
        }

        #[test]
        fn format_line_comment_on_multi_line_expression() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![Expression::Symbol("foo", Position::new(6, 9))],
                        Position::new(0, 10)
                    )],
                    &[LineComment::new("bar", Position::new(1, 5)).into()],
                    &[],
                    "(;bar\nfoo)",
                )
                .unwrap(),
                indoc!(
                    "
                    ( ;bar
                     foo)
                    "
                )
            );
        }

        #[test]
        fn format_line_comment_for_last_argument_in_different_line() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("f", Position::new(0, 1)),
                            Expression::Symbol("x", Position::new(2, 3))
                        ],
                        Position::new(0, 1)
                    )],
                    &[LineComment::new("foo", Position::new(1, 2)).into()],
                    &[],
                    "\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    (f
                     ;foo
                     x)
                    "
                )
            );
        }

        #[test]
        fn format_remaining_multi_line_comment() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(0, 1))],
                    &[LineComment::new("bar", Position::new(1, 2)).into()],
                    &[],
                    "\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    foo

                    ;bar
                    "
                )
            );
        }

        #[test]
        fn format_remaining_multi_line_comments() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(0, 1))],
                    &[
                        LineComment::new("bar", Position::new(1, 2)).into(),
                        LineComment::new("baz", Position::new(2, 3)).into()
                    ],
                    &[],
                    "\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    foo

                    ;bar
                    ;baz
                    "
                )
            );
        }

        #[test]
        fn format_remaining_multi_line_comments_with_extra_line() {
            assert_eq!(
                format(
                    &[Expression::Symbol("foo", Position::new(0, 1))],
                    &[
                        LineComment::new("bar", Position::new(1, 2)).into(),
                        LineComment::new("baz", Position::new(3, 4)).into()
                    ],
                    &[],
                    "\n\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    foo

                    ;bar

                    ;baz
                    "
                )
            );
        }

        #[test]
        fn format_line_comment_before_closing_parenthesis() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::Symbol("bar", Position::new(1, 2))
                        ],
                        Position::new(0, 5)
                    )],
                    &[
                        LineComment::new("baz", Position::new(2, 3)).into(),
                        LineComment::new("qux", Position::new(3, 4)).into()
                    ],
                    &[],
                    "\n\n\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo
                     bar
                     ;baz
                     ;qux
                     )
                    "
                )
            );
        }

        #[test]
        fn format_line_and_inline_comments_before_closing_parenthesis() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::Symbol("bar", Position::new(1, 2))
                        ],
                        Position::new(0, 6)
                    )],
                    &[
                        LineComment::new("baz", Position::new(2, 3)).into(),
                        LineComment::new("qux", Position::new(3, 4)).into(),
                        BlockComment::new("quux", Position::new(4, 5)).into(),
                        BlockComment::new("blah", Position::new(4, 5)).into(),
                    ],
                    &[],
                    "\n\n\n\n)\n",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo
                     bar
                     ;baz
                     ;qux
                     #|quux|# #|blah|#)
                    "
                )
            );
        }

        #[test]
        fn format_line_and_inline_comments_before_closing_parenthesis_in_nested_expression() {
            assert_eq!(
                format(
                    &[Expression::List(
                        "(",
                        ")",
                        vec![
                            Expression::Symbol("foo", Position::new(0, 1)),
                            Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("foo", Position::new(0, 1)),
                                    Expression::Symbol("bar", Position::new(1, 2))
                                ],
                                Position::new(0, 6)
                            )
                        ],
                        Position::new(0, 6)
                    )],
                    &[
                        LineComment::new("baz", Position::new(2, 3)).into(),
                        LineComment::new("qux", Position::new(3, 4)).into(),
                        BlockComment::new("quux", Position::new(4, 5)).into(),
                        BlockComment::new("blah", Position::new(4, 5)).into(),
                    ],
                    &[],
                    "\n\n\n\n)\n",
                )
                .unwrap(),
                indoc!(
                    "
                    (foo (foo
                          bar
                          ;baz
                          ;qux
                          #|quux|# #|blah|#))
                    "
                )
            );
        }

        #[test]
        fn format_blank_lines_before_terminal_line_comments() {
            for end in [3, 4] {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(1, 2)),
                            ],
                            Position::new(0, end + 2)
                        )],
                        &[LineComment::new("baz", Position::new(end, end + 1)).into()],
                        &[],
                        "\n\n\n\n\n\n",
                    )
                    .unwrap(),
                    indoc!(
                        "
                    (foo
                     bar

                     ;baz
                     )
                    "
                    )
                );
            }
        }

        mod suffix {
            use super::*;
            use pretty_assertions::assert_eq;

            #[test]
            fn format_same_line() {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(0, 1))
                            ],
                            Position::new(0, 1)
                        )],
                        &[LineComment::new("baz", Position::new(0, 1)).into()],
                        &[],
                        "\n",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        (foo bar) ;baz
                        "
                    )
                );
            }

            #[test]
            fn format_next_line() {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(1, 2))
                            ],
                            Position::new(0, 2)
                        )],
                        &[LineComment::new("baz", Position::new(1, 2)).into()],
                        &[],
                        "\n\n",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        (foo
                         bar) ;baz
                        "
                    )
                );
            }

            #[test]
            fn format_next_next_line() {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(2, 3))
                            ],
                            Position::new(0, 3)
                        )],
                        &[LineComment::new("baz", Position::new(2, 3)).into()],
                        &[],
                        "\n\n\n",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        (foo

                         bar) ;baz
                        "
                    )
                );
            }

            #[test]
            fn format_next_next_line_with_inline_comment() {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![Expression::Symbol("foo", Position::new(0, 1))],
                            Position::new(0, 4)
                        )],
                        &[
                            BlockComment::new("bar", Position::new(2, 3)).into(),
                            LineComment::new("baz", Position::new(4, 5)).into()
                        ],
                        &[],
                        "\n\na)b",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        (foo #|bar|#) ;baz
                        "
                    )
                );
            }

            #[test]
            fn format_next_next_line_with_expression_and_inline_comment() {
                assert_eq!(
                    format(
                        &[Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(1, 2))
                            ],
                            Position::new(0, 4)
                        )],
                        &[
                            BlockComment::new("bar", Position::new(2, 3)).into(),
                            LineComment::new("baz", Position::new(4, 5)).into()
                        ],
                        &[],
                        "\n\na)b",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        (foo
                         bar #|bar|#) ;baz
                        "
                    )
                );
            }
        }

        mod block {
            use super::*;

            mod inline {
                use super::*;
                use pretty_assertions::assert_eq;

                #[test]
                fn format_in_front() {
                    assert_eq!(
                        format(
                            &[Expression::Symbol("bar", Position::new(7, 10))],
                            &[BlockComment::new("foo", Position::new(0, 7)).into(),],
                            &[],
                            "#|foo|#bar",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            #|foo|# bar
                            "
                        )
                    );
                }

                #[test]
                fn format_after_first_expression() {
                    assert_eq!(
                        format(
                            &[Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("foo", Position::new(1, 4)),
                                    Expression::Symbol("baz", Position::new(11, 14))
                                ],
                                Position::new(0, 15)
                            ),],
                            &[BlockComment::new("bar", Position::new(4, 11)).into(),],
                            &[],
                            "(foo#|bar|#baz)",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            (foo #|bar|# baz)
                            "
                        )
                    );
                }

                #[test]
                fn format_after_second_expression() {
                    assert_eq!(
                        format(
                            &[Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("foo", Position::new(1, 4)),
                                    Expression::Symbol("bar", Position::new(5, 8)),
                                    Expression::Symbol("qux", Position::new(15, 18)),
                                ],
                                Position::new(0, 19)
                            ),],
                            &[BlockComment::new("baz", Position::new(8, 15)).into(),],
                            &[],
                            "(foo bar#|baz|#qux)",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            (foo bar #|baz|# qux)
                            "
                        )
                    );
                }

                #[test]
                fn format_after_third_expression() {
                    assert_eq!(
                        format(
                            &[Expression::List(
                                "(",
                                ")",
                                vec![
                                    Expression::Symbol("foo", Position::new(1, 4)),
                                    Expression::Symbol("bar", Position::new(5, 8)),
                                    Expression::Symbol("qux", Position::new(9, 12)),
                                ],
                                Position::new(0, 20)
                            ),],
                            &[BlockComment::new("baz", Position::new(12, 19)).into(),],
                            &[],
                            "(foo bar qux#|baz|#)",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            (foo bar qux #|baz|#)
                            "
                        )
                    );
                }

                #[test]
                fn format_in_empty_list() {
                    assert_eq!(
                        format(
                            &[Expression::List("(", ")", vec![], Position::new(0, 9))],
                            &[BlockComment::new("foo", Position::new(1, 8)).into(),],
                            &[],
                            "(#|foo|#)",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            (#|foo|#)
                            "
                        )
                    );
                }

                #[test]
                fn format_many() {
                    assert_eq!(
                        format(
                            &[Expression::List("(", ")", vec![], Position::new(0, 2))],
                            &[
                                BlockComment::new("foo", Position::new(0, 1)).into(),
                                BlockComment::new("bar", Position::new(0, 1)).into(),
                                BlockComment::new("baz", Position::new(0, 1)).into(),
                            ],
                            &[],
                            "a\n",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            (#|foo|# #|bar|# #|baz|#)
                            "
                        )
                    );
                }
            }

            mod multi_line {
                use super::*;
                use pretty_assertions::assert_eq;

                #[test]
                fn format_with_no_blank_line() {
                    assert_eq!(
                        format(
                            &[
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(12, 13))
                            ],
                            &[BlockComment::new("foo", Position::new(4, 5)).into(),],
                            &[],
                            "foo\n#|foo|#\nbar",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            foo
                            #|
                            foo
                            |#
                            bar
                            "
                        )
                    );
                }

                #[test]
                fn format_with_blank_lines() {
                    assert_eq!(
                        format(
                            &[
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(14, 15))
                            ],
                            &[BlockComment::new("foo", Position::new(5, 6)).into(),],
                            &[],
                            "foo\n\n#|foo|#\n\nbar",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            foo

                            #|
                            foo
                            |#

                            bar
                            "
                        )
                    );
                }

                #[test]
                fn format_with_extra_blank_lines() {
                    assert_eq!(
                        format(
                            &[
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(16, 17))
                            ],
                            &[BlockComment::new("foo", Position::new(6, 7)).into(),],
                            &[],
                            "foo\n\n\n#|foo|#\n\n\nbar",
                        )
                        .unwrap(),
                        indoc!(
                            "
                            foo

                            #|
                            foo
                            |#

                            bar
                            "
                        )
                    );
                }
            }
        }
    }

    mod hash_directive {
        use super::*;
        use pretty_assertions::assert_eq;

        #[test]
        fn format_hash_directive() {
            assert_eq!(
                format(
                    &[],
                    &[],
                    &[HashDirective::new("foo", Position::new(0, 0))],
                    "\n",
                )
                .unwrap(),
                indoc!(
                    "
                    #foo
                    "
                )
            );
        }

        #[test]
        fn format_hash_directives() {
            assert_eq!(
                format(
                    &[],
                    &[],
                    &[
                        HashDirective::new("foo", Position::new(0, 0)),
                        HashDirective::new("bar", Position::new(2, 2))
                    ],
                    "\n\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    #foo
                    #bar
                    "
                )
            );
        }

        #[test]
        fn format_hash_directive_with_expression() {
            assert_eq!(
                format(
                    &[Expression::Symbol("bar", Position::new(0, 1))],
                    &[],
                    &[HashDirective::new("foo", Position::new(1, 2))],
                    "\n\n",
                )
                .unwrap(),
                indoc!(
                    "
                    #foo

                    bar
                    "
                )
            );
        }
    }

    mod data {
        use super::*;
        use pretty_assertions::assert_eq;

        #[test]
        fn format_list() {
            assert_eq!(
                format(
                    &[Expression::Quote(
                        "'",
                        Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(0, 1))
                            ],
                            Position::new(0, 1)
                        )
                        .into(),
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "(foo bar)",
                )
                .unwrap(),
                "'(foo bar)\n"
            );
        }

        #[test]
        fn format_broken_list() {
            assert_eq!(
                format(
                    &[Expression::Quote(
                        "'",
                        Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(0, 1)),
                                Expression::Symbol("bar", Position::new(6, 7))
                            ],
                            Position::new(0, 1)
                        )
                        .into(),
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "'(foo\nbar)",
                )
                .unwrap(),
                indoc!(
                    "
                    '(foo
                      bar)
                    "
                )
            );
        }

        #[test]
        fn format_broken_list_with_empty_line() {
            assert_eq!(
                format(
                    &[Expression::Quote(
                        "'",
                        Expression::List(
                            "(",
                            ")",
                            vec![
                                Expression::Symbol("foo", Position::new(3, 4)),
                                Expression::Symbol("bar", Position::new(3, 4))
                            ],
                            Position::new(0, 1)
                        )
                        .into(),
                        Position::new(0, 1)
                    )],
                    &[],
                    &[],
                    "'(\nfoo\nbar)",
                )
                .unwrap(),
                indoc!(
                    "
                    '(
                      foo
                      bar)
                    "
                )
            );
        }

        #[test]
        fn format_line_comment_on_multi_line_expression() {
            assert_eq!(
                format(
                    &[Expression::Quote(
                        "'",
                        Expression::List(
                            "(",
                            ")",
                            vec![Expression::Symbol("foo", Position::new(7, 10))],
                            Position::new(1, 11)
                        )
                        .into(),
                        Position::new(0, 11)
                    )],
                    &[LineComment::new("bar", Position::new(2, 6)).into()],
                    &[],
                    "'(;bar\nfoo)",
                )
                .unwrap(),
                indoc!(
                    "
                    '( ;bar
                      foo)
                    "
                )
            );
        }

        #[test]
        fn format_first_element_on_second_line() {
            assert_eq!(
                format(
                    &[Expression::Quote(
                        "'",
                        Expression::List(
                            "(",
                            ")",
                            vec![Expression::Symbol("foo", Position::new(3, 6))],
                            Position::new(1, 7)
                        )
                        .into(),
                        Position::new(0, 7)
                    )],
                    &[],
                    &[],
                    "'(\nfoo)",
                )
                .unwrap(),
                indoc!(
                    "
                    '(
                      foo)
                    "
                )
            );
        }

        mod nested {
            use super::*;
            use pretty_assertions::assert_eq;

            #[test]
            fn format_nested_lists() {
                assert_eq!(
                    format(
                        &[Expression::Quote(
                            "'",
                            Expression::List(
                                "(",
                                ")",
                                vec![Expression::List(
                                    "(",
                                    ")",
                                    vec![
                                        Expression::Symbol("foo", Position::new(0, 1)),
                                        Expression::Symbol("bar", Position::new(0, 1))
                                    ],
                                    Position::new(0, 1)
                                ),],
                                Position::new(0, 1)
                            )
                            .into(),
                            Position::new(0, 1)
                        )],
                        &[],
                        &[],
                        "'((foo bar))",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        '((foo bar))
                        "
                    )
                );
            }

            #[test]
            fn format_broken_nested_lists() {
                assert_eq!(
                    format(
                        &[Expression::Quote(
                            "'",
                            Expression::List(
                                "(",
                                ")",
                                vec![Expression::List(
                                    "(",
                                    ")",
                                    vec![
                                        Expression::Symbol("foo", Position::new(0, 1)),
                                        Expression::Symbol("bar", Position::new(7, 8))
                                    ],
                                    Position::new(0, 1)
                                ),],
                                Position::new(0, 1)
                            )
                            .into(),
                            Position::new(0, 1)
                        )],
                        &[],
                        &[],
                        "'((foo\nbar))",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        '((foo
                           bar))
                        "
                    )
                );
            }

            #[test]
            fn format_nested_unquote() {
                assert_eq!(
                    format(
                        &[Expression::Quote(
                            ",",
                            Expression::List(
                                "(",
                                ")",
                                vec![Expression::List(
                                    "(",
                                    ")",
                                    vec![
                                        Expression::Symbol("foo", Position::new(0, 1)),
                                        Expression::Symbol("bar", Position::new(7, 8))
                                    ],
                                    Position::new(0, 1)
                                ),],
                                Position::new(0, 1)
                            )
                            .into(),
                            Position::new(0, 1)
                        )],
                        &[],
                        &[],
                        ",((foo\nbar))",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        ,((foo
                           bar))
                        "
                    )
                );
            }

            #[test]
            fn format_splicing_unquote() {
                assert_eq!(
                    format(
                        &[Expression::Quote(
                            "'",
                            Expression::Quote(
                                ",@",
                                Expression::List(
                                    "(",
                                    ")",
                                    vec![
                                        Expression::Symbol("foo", Position::new(4, 7)),
                                        Expression::List(
                                            "(",
                                            ")",
                                            vec![
                                                Expression::Symbol("bar", Position::new(9, 12)),
                                                Expression::Symbol("baz", Position::new(13, 16)),
                                            ],
                                            Position::new(8, 17)
                                        ),
                                    ],
                                    Position::new(3, 18)
                                )
                                .into(),
                                Position::new(1, 18)
                            )
                            .into(),
                            Position::new(0, 18)
                        )],
                        &[],
                        &[],
                        "',@(foo\n(bar\nbaz))",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        ',@(foo
                            (bar
                              baz))
                        "
                    )
                );
            }

            #[test]
            fn format_split_splicing_unquote() {
                assert_eq!(
                    format(
                        &[Expression::Quote(
                            "'",
                            Expression::Quote(
                                ",",
                                Expression::Quote(
                                    "@",
                                    Expression::List(
                                        "(",
                                        ")",
                                        vec![
                                            Expression::Symbol("foo", Position::new(4, 7)),
                                            Expression::List(
                                                "(",
                                                ")",
                                                vec![
                                                    Expression::Symbol("bar", Position::new(9, 12)),
                                                    Expression::Symbol(
                                                        "baz",
                                                        Position::new(13, 16)
                                                    ),
                                                ],
                                                Position::new(8, 17)
                                            ),
                                        ],
                                        Position::new(3, 18)
                                    )
                                    .into(),
                                    Position::new(2, 18)
                                )
                                .into(),
                                Position::new(1, 18)
                            )
                            .into(),
                            Position::new(0, 18)
                        )],
                        &[],
                        &[],
                        "',@(foo\n(bar\nbaz))",
                    )
                    .unwrap(),
                    indoc!(
                        "
                        ',@(foo
                            (bar
                              baz))
                        "
                    )
                );
            }
        }
    }
}
