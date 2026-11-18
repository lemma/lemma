; Lemma highlights. Later patterns take precedence in Zed.
; Keywords are case-insensitive (engine lexer: eq_ignore_ascii_case), so
; identifiers are classified with (?i) regexes. Keep the word lists in sync
; with COMPLETION_KEYWORD_LABELS and TypeConstraintCommand (drift test:
; engine/lsp/src/zed_drift.rs).

(identifier) @variable

(commentary) @comment.doc
(string) @string
(number) @number
(datetime) @constant
(time) @constant
(registry_name) @namespace
(operator) @operator
(punctuation) @punctuation

[
  "spec"
  "repo"
  "data"
  "rule"
  "meta"
] @keyword

(spec_header name: (logical_name) @type)
(repo_header name: (logical_name) @namespace)
(data_header name: (data_path) @property)
(rule_header name: (identifier) @function)
(meta_header name: (identifier) @property)

((identifier) @keyword
  (#match? @keyword "^(?i)(uses|with)$"))

((identifier) @keyword.control
  (#match? @keyword.control "^(?i)(unless|then|that|not|and|or|in|as|is|mod|veto|now|past|future|effective|permille|calendar)$"))

((identifier) @type
  (#match? @type "^(?i)(boolean|measure|number|ratio|text|date|time|range|list)$"))

((identifier) @boolean
  (#match? @boolean "^(?i)(true|false|yes|no)$"))

((identifier) @function.builtin
  (#match? @function.builtin "^(?i)(sqrt|sin|cos|tan|asin|acos|atan|log|exp|abs|floor|ceil|round)$"))

((identifier) @keyword.operator
  (#match? @keyword.operator "^(?i)(help|suggest|unit|trait|minimum|maximum|lower|upper|decimals|option|options|length|key|find|first|last|sum|avg|product|count|min|max|any|all|where|sort|top|asc|desc)$"))
