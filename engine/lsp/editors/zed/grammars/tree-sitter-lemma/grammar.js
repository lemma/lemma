/**
 * Lexical tree-sitter grammar for Lemma.
 *
 * Scope: token-level highlighting plus declaration headers (spec, repo, data,
 * rule, meta) for outline. The engine parser (engine/src/parsing/parser.rs) is
 * authoritative for structure and validity; full coloring comes from the Lemma
 * LSP semantic tokens. Token shapes mirror engine/src/parsing/lexer.rs.
 *
 * Keywords are case-insensitive in Lemma (`keyword_from_identifier` uses
 * eq_ignore_ascii_case), so non-declaration keywords lex as `identifier` and
 * are classified in queries with case-insensitive `#match?` predicates.
 */

/**
 * Case-insensitive keyword token, exposed as the lowercase anonymous node `word`.
 * @param {string} word
 */
function keyword(word) {
  const pattern = word
    .split("")
    .map((c) => `[${c.toLowerCase()}${c.toUpperCase()}]`)
    .join("");
  return alias(token(prec(1, new RegExp(pattern))), word);
}

module.exports = grammar({
  name: "lemma",

  extras: ($) => [/\s/],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.spec_header,
        $.repo_header,
        $.data_header,
        $.rule_header,
        $.meta_header,
        $.commentary,
        $.string,
        $.datetime,
        $.time,
        $.number,
        $.registry_name,
        $.identifier,
        $.operator,
        $.punctuation,
      ),

    // `spec <logical name>`; an optional effective datetime follows as flat tokens.
    spec_header: ($) => seq(keyword("spec"), field("name", $.logical_name)),

    // `repo <logical name>`; registry qualifiers keep their leading `@`.
    repo_header: ($) => seq(keyword("repo"), field("name", $.logical_name)),

    // `data <path>`; the colon and body follow as flat tokens.
    data_header: ($) => seq(keyword("data"), field("name", $.data_path)),

    // `rule <name>`; the colon and body follow as flat tokens.
    rule_header: ($) => seq(keyword("rule"), field("name", $.identifier)),

    // `meta <key>`; the colon and literal follow as flat tokens.
    meta_header: ($) => seq(keyword("meta"), field("name", $.identifier)),

    // Opaque spec / repo name: `[A-Za-z0-9_./-]` characters, optional leading `@`.
    logical_name: (_) => /@?[A-Za-z0-9_./-]+/,

    data_path: (_) => /[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*/,

    registry_name: (_) => /@[A-Za-z0-9_./-]+/,

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    // Triple-quoted commentary block: the only documentation form in Lemma.
    commentary: (_) => token(seq('"""', /([^"]|"[^"]|""[^"])*/, '"""')),

    string: (_) => /"[^"]*"/,

    datetime: (_) =>
      /\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}([.,]\d+)?)?(Z|[+-]\d{2}:\d{2})?)?/,

    time: (_) => /\d{2}:\d{2}(:\d{2}([.,]\d+)?)?(Z|[+-]\d{2}:\d{2})?/,

    number: (_) => /\d[\d_]*(\.\d+)?([eE][+-]?\d+)?/,

    operator: (_) =>
      choice("->", ">=", "<=", "%%", "...", "+", "-", "*", "/", "^", ">", "<", "%"),

    punctuation: (_) => choice(":", ",", ".", "(", ")"),
  },
});
