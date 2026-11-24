/**
 * tree-sitter grammar for zustscript (crates/zustscript), for the playground's highlighting.
 *
 * It follows src/parse.rs: items (`type`, `enum`, `fn`, `extern`, `import type`, `wasm { .. }`, `wasm fn`), blocks,
 * patterns, expressions with `|>`, arrow closures and JSX. The Rust of `wasm { .. }` / `wasm fn` bodies is read as
 * tokens in balanced brackets (enough to color it).
 */

const PREC = { closure: 0, pipe: 1, or: 2, and: 3, cmp: 4, add: 5, mul: 6, cast: 7, unary: 8, postfix: 9 };

const sep = (rule, s) => optional(seq(rule, repeat(seq(s, rule)), optional(s)));
const commaSep = (rule) => sep(rule, ',');

export default grammar({
  name: 'zustscript',

  word: ($) => $.identifier,

  extras: ($) => [/\s/, $.line_comment, $.block_comment],

  conflicts: ($) => [
    [$.constructor, $.record_expression],
    [$.closure_parameter, $._expression],
  ],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.type_item,
        $.enum_item,
        $.function_item,
        $.extern_item,
        $.import_item,
        $.wasm_block,
        $.wasm_function_item,
      ),

    attribute: ($) => seq('@', field('name', alias(choice('wasm', 'simd'), $.attribute_name)), optional(seq('(', $.identifier, ')'))),
    visibility: (_) => 'pub',

    type_item: ($) =>
      seq(optional($.visibility), 'type', field('name', $.type_identifier), optional($.type_parameters), '=', $.record_type),

    record_type: ($) => seq('{', commaSep($.field_declaration), '}'),
    field_declaration: ($) => seq(field('name', $.field_identifier), ':', field('type', $._type)),

    enum_item: ($) =>
      seq(optional($.visibility), 'enum', field('name', $.type_identifier), optional($.type_parameters), '{', commaSep($.variant), '}'),
    variant: ($) =>
      seq(field('name', $._any_variant), optional(seq('(', commaSep($._type), ')')), optional(seq('->', field('result', $._type)))),

    function_item: ($) =>
      seq(
        optional($.attribute),
        optional($.visibility),
        'fn',
        field('name', $.function_identifier),
        optional($.type_parameters),
        $.parameters,
        optional(seq('->', field('result', $._type))),
        field('body', $.block),
      ),
    type_parameters: ($) => seq('<', commaSep(seq($.type_identifier, optional(seq(':', $.type_identifier)))), '>'),
    parameters: ($) => seq('(', commaSep($.parameter), ')'),
    parameter: ($) => seq(field('name', $.identifier), ':', field('type', $._type)),

    extern_item: ($) =>
      seq(
        'extern',
        optional(field('module', $.string)),
        'fn',
        field('name', $.function_identifier),
        optional($.type_parameters),
        $.parameters,
        optional(seq('->', field('result', $._type))),
        optional(seq('=', field('js', $.string))),
        ';',
      ),

    import_item: ($) =>
      seq('import', 'type', '{', commaSep($.type_identifier), '}', 'from', field('path', $.string), ';'),

    // ---- the wasm layer: Rust, as tokens
    wasm_block: ($) => seq('wasm', alias($.rust_braces, $.rust_block)),
    wasm_function_item: ($) =>
      seq(
        optional($.visibility),
        'wasm',
        'fn',
        field('name', $.function_identifier),
        alias($.rust_parens, $.rust_parameters),
        optional(seq('->', alias(repeat1($._rust_type_token), $.rust_type))),
        field('body', alias($.rust_braces, $.rust_block)),
      ),
    rust_braces: ($) => seq('{', repeat($._rust_token), '}'),
    rust_parens: ($) => seq('(', repeat($._rust_token), ')'),
    rust_brackets: ($) => seq('[', repeat($._rust_token), ']'),
    _rust_type_token: ($) =>
      choice($.primitive_type, alias($.identifier, $.type_identifier), $.rust_keyword, alias($.rust_parens, $.rust_group), alias($.rust_brackets, $.rust_group), $.rust_punct, '<', '>'),
    _rust_token: ($) =>
      choice(
        $.rust_braces,
        $.rust_parens,
        $.rust_brackets,
        $.rust_keyword,
        $.primitive_type,
        $.boolean,
        $.number,
        $.string,
        $.char,
        $.rust_macro,
        $.identifier,
        $.rust_punct,
        '<',
        '>',
        ';',
        ',',
        ':',
        '=',
        '->',
        '=>',
        '.',
      ),
    rust_macro: ($) => seq(field('name', $.identifier), '!'),
    rust_keyword: (_) =>
      choice('let', 'mut', 'fn', 'for', 'in', 'if', 'else', 'return', 'as', 'while', 'loop', 'match', 'struct', 'impl', 'pub', 'use', 'const', 'static', 'dyn', 'self', 'Self', 'break', 'continue', 'where', 'trait', 'enum', 'type', 'unsafe', 'ref', 'move', 'mod', 'crate', 'super'),
    rust_punct: (_) => token(choice('::', '..', '..=', '&&', '||', '==', '!=', '<=', '>=', '+=', '-=', '*=', '/=', '%=', '&', '|', '+', '-', '*', '/', '%', '!', '?', '^', '#', '@', "'")),
    char: (_) => token(seq("'", choice(/[^'\\]/, /\\./, /\\u\{[0-9a-fA-F]+\}/), "'")),

    // ---- types
    _type: ($) => choice($.primitive_type, $.type_identifier, $.generic_type, $.function_type, $.tuple_type, $.unit_type),
    primitive_type: (_) => choice('i8', 'i16', 'i32', 'i64', 'u8', 'u16', 'u32', 'u64', 'usize', 'isize', 'f32', 'f64', 'bool', 'String', 'str'),
    generic_type: ($) => seq(field('name', $.type_identifier), '<', commaSep($._type), '>'),
    function_type: ($) => prec.right(seq('fn', '(', commaSep($._type), ')', optional(seq('->', $._type)))),
    tuple_type: ($) => seq('(', $._type, ',', commaSep($._type), ')'),
    unit_type: (_) => seq('(', ')'),

    // ---- statements
    block: ($) => seq('{', repeat($._statement), optional(field('tail', $._expression)), '}'),
    _statement: ($) => choice($.let_statement, $.assignment_statement, $.expression_statement, $.empty_statement),
    empty_statement: (_) => ';',
    let_statement: ($) =>
      seq('let', optional($.mutable), field('pattern', $.pattern), optional(seq(':', field('type', $._type))), '=', field('value', $._expression), ';'),
    mutable: (_) => 'mut',
    assignment_statement: ($) =>
      seq(field('left', $._expression), field('operator', choice('=', '+=', '-=', '*=', '/=', '%=')), field('right', $._expression), ';'),
    expression_statement: ($) => choice(seq($._expression, ';'), prec(1, $._block_like)),
    _block_like: ($) => choice($.if_expression, $.match_expression, $.while_expression, $.for_expression, $.block, $.handle_expression),

    // ---- patterns
    pattern: ($) =>
      choice(
        $.wildcard,
        $.identifier,
        $.boolean,
        $.number,
        $.string,
        seq('-', $.number),
        $.tuple_pattern,
        $.variant_pattern,
      ),
    wildcard: (_) => '_',
    tuple_pattern: ($) => seq('(', commaSep($.pattern), ')'),
    variant_pattern: ($) =>
      prec.right(seq(optional(seq(alias($._upper, $.type_identifier), '::')), field('name', $.variant_identifier), optional(seq('(', commaSep($.pattern), ')')))),

    // ---- expressions
    _expression: ($) =>
      choice(
        $.identifier,
        $.constructor,
        $.path_expression,
        $.number,
        $.string,
        $.boolean,
        $.unit,
        $.parenthesized_expression,
        $.tuple_expression,
        $.array_expression,
        $.record_expression,
        $.call_expression,
        $.field_expression,
        $.index_expression,
        $.unary_expression,
        $.binary_expression,
        $.pipe_expression,
        $.cast_expression,
        $.closure_expression,
        $.jsx_element,
        $.jsx_self_closing_element,
        $.jsx_fragment,
        $.perform_expression,
        $.break_expression,
        $.continue_expression,
        $.return_expression,
        $._block_like,
      ),

    constructor: ($) => $._upper,
    path_expression: ($) => seq(field('type', alias($._upper, $.type_identifier)), '::', field('name', choice($.identifier, alias($._upper, $.variant_identifier)))),
    unit: (_) => seq('(', ')'),
    parenthesized_expression: ($) => seq('(', $._expression, ')'),
    tuple_expression: ($) => seq('(', $._expression, ',', commaSep($._expression), ')'),
    array_expression: ($) => seq('[', commaSep($._expression), ']'),
    record_expression: ($) =>
      seq(field('name', alias($._upper, $.type_identifier)), '{', commaSep(choice($.field_initializer, $.base_initializer, alias($.identifier, $.shorthand_field))), '}'),
    field_initializer: ($) => seq(field('name', $.field_identifier), ':', field('value', $._expression)),
    base_initializer: ($) => seq('..', $._expression),

    arguments: ($) => seq('(', commaSep($._expression), ')'),
    call_expression: ($) => prec(PREC.postfix, seq(field('function', $._expression), $.arguments)),
    field_expression: ($) =>
      prec(PREC.postfix, seq(field('value', $._expression), '.', field('field', choice(alias($.identifier, $.field_identifier), $.number)))),
    index_expression: ($) => prec(PREC.postfix, seq($._expression, '[', $._expression, ']')),
    unary_expression: ($) => prec(PREC.unary, seq(field('operator', choice('-', '!', '*', '&')), $._expression)),
    cast_expression: ($) => prec.left(PREC.cast, seq($._expression, 'as', choice($.primitive_type, $.type_identifier))),
    binary_expression: ($) =>
      choice(
        ...[
          [PREC.or, '||'],
          [PREC.and, '&&'],
          [PREC.cmp, choice('==', '!=', '<', '<=', '>', '>=')],
          [PREC.add, choice('+', '-')],
          [PREC.mul, choice('*', '/', '%')],
        ].map(([p, op]) => prec.left(p, seq(field('left', $._expression), field('operator', op), field('right', $._expression)))),
      ),
    pipe_expression: ($) =>
      prec.left(
        PREC.pipe,
        seq(
          field('left', $._expression),
          '|>',
          field('right', choice(seq('.', field('method', alias($.identifier, $.method_identifier)), $.arguments), $._expression)),
        ),
      ),
    closure_expression: ($) => prec.right(PREC.closure, seq($.closure_parameters, '=>', field('body', $._expression))),
    closure_parameters: ($) => seq('(', commaSep($.closure_parameter), ')'),
    closure_parameter: ($) => seq($.identifier, optional(seq(':', $._type))),

    if_expression: ($) =>
      prec.right(seq('if', field('condition', $._expression), field('consequence', $.block), optional(seq('else', field('alternative', choice($.block, $.if_expression)))))),
    while_expression: ($) => seq('while', field('condition', $._expression), field('body', $.block)),
    for_expression: ($) => seq('for', field('binding', $.identifier), 'in', field('iterable', $._expression), field('body', $.block)),
    match_expression: ($) => seq('match', field('value', $._expression), '{', commaSep($.match_arm), '}'),
    match_arm: ($) => seq(field('pattern', $.pattern), '=>', field('body', $._expression)),
    handle_expression: ($) => seq('handle', field('body', $._expression), '{', commaSep($.handler_arm), '}'),
    handler_arm: ($) =>
      seq(field('operation', $._any_variant), optional(seq('(', commaSep($.pattern), ')')), ',', field('continuation', $.identifier), '=>', field('body', $._expression)),
    perform_expression: ($) =>
      prec.right(seq('perform', optional(seq(alias($._upper, $.type_identifier), '::')), field('operation', $._any_variant), optional($.arguments))),
    break_expression: (_) => 'break',
    continue_expression: (_) => 'continue',
    return_expression: ($) => prec.right(seq('return', optional($._expression))),

    // ---- JSX
    jsx_element: ($) => seq($.jsx_opening_element, repeat($._jsx_child), $.jsx_closing_element),
    jsx_fragment: ($) => seq('<', '>', repeat($._jsx_child), '<', '/', '>'),
    jsx_opening_element: ($) => seq('<', field('name', $.jsx_tag), repeat($.jsx_attribute), '>'),
    jsx_closing_element: ($) => seq('<', '/', field('name', $.jsx_tag), '>'),
    jsx_self_closing_element: ($) => seq('<', field('name', $.jsx_tag), repeat($.jsx_attribute), '/', '>'),
    jsx_tag: ($) => seq($.identifier, repeat(seq('.', $.identifier))),
    jsx_attribute: ($) =>
      seq(field('name', alias($.identifier, $.jsx_attribute_name)), optional(seq('=', field('value', choice($.string, $.jsx_expression))))),
    jsx_expression: ($) => seq('{', optional($._expression), '}'),
    _jsx_child: ($) => choice($.jsx_text, $.jsx_expression, $.jsx_element, $.jsx_self_closing_element),
    jsx_text: (_) => token(prec(-1, /[^{}<>]+/)),

    // ---- names and literals
    identifier: (_) => /[\p{L}_][\p{L}\p{N}_]*/u,
    _upper: (_) => token(prec(1, /\p{Lu}[\p{L}\p{N}_]*/u)),
    type_identifier: ($) => alias($.identifier, $.type_identifier),
    field_identifier: ($) => alias($.identifier, $.field_identifier),
    function_identifier: ($) => alias($.identifier, $.function_identifier),
    variant_identifier: ($) => alias($._upper, $.variant_identifier),
    // (an operation of an effect may be lowercase: `perform toString`)
    _any_variant: ($) => choice($.variant_identifier, alias($.identifier, $.variant_identifier)),

    boolean: (_) => choice('true', 'false'),
    number: (_) => token(seq(/[0-9][0-9_]*/, optional(seq('.', /[0-9][0-9_]*/)), optional(seq(/[eE][+-]?/, /[0-9]+/)), optional(/(i8|i16|i32|i64|u8|u16|u32|u64|usize|isize|f32|f64)/))),
    string: (_) => token(seq('"', repeat(choice(/[^"\\]/, /\\./)), '"')),

    line_comment: (_) => token(seq('//', /[^\n]*/)),
    block_comment: (_) => token(seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/')),
  },
});
