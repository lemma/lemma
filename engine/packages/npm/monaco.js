/**
 * Monaco Editor language registration for Lemma.
 * Syntax highlighting is provided by the Rust LSP via semantic tokens.
 */

// Must stay in exact index order with TOKEN_TYPES in engine/lsp/src/semantic_tokens.rs.
export const SEMANTIC_TOKEN_TYPES = [
  'namespace',          // 0 — repo qualifier
  'class',              // 1 — spec name
  'property',           // 2 — data field path (before colon)
  'function',           // 3 — rule name (colon is punctuation)
  'string',             // 4 — values in rule body / defaults
  'comment',            // 5
  'keyword',            // 6 — type keywords, math functions, constraint words
  'operator',           // 7
  'controlKeyword',     // 8 — unless, then, not, and, in, type, veto, now, past, future, stray repo
  'dataBody',           // 9 — data block after the colon
  'punctuation',        // 10 — colons after data field and rule name
  'reference',          // 11 — identifiers in spec/rule body
  'declarationKeyword', // 12 — declaration keywords: spec, data, with, rule, repo, uses, meta
];

export const SEMANTIC_TOKEN_MODIFIERS = [];

/**
 * Monaco theme rules for Lemma semantic tokens.
 *
 * Monaco standalone resolves semantic token colors via its TextMate rule
 * matcher: getTokenStyleMetadata calls _match(type + '.' + modifier).
 * Rules must therefore use the token type name (with optional .modifier suffix)
 * and foreground values WITHOUT a leading '#'.
 *
 * Use in monaco.editor.defineTheme: { ..., rules: LEMMA_MONACO_RULES }
 */
export const LEMMA_MONACO_RULES = [
  { token: 'namespace',          foreground: 'BD8EBB' }, // repo qualifier
  { token: 'class',              foreground: '5DBDAA' }, // spec name
  { token: 'property',           foreground: 'A3B5DF' }, // data field path
  { token: 'function',           foreground: 'A3B5DF' }, // rule name
  { token: 'string',             foreground: 'D07868' }, // same as dataBody (values / literals bucket)
  { token: 'comment',            foreground: '595945' },
  { token: 'keyword',            foreground: '6BA0C2' }, // type + math keywords + constraint words
  { token: 'operator',           foreground: '80807A' },
  { token: 'controlKeyword',     foreground: '726B83' }, // unless, then, not, and, in, type, veto, now, past, future
  { token: 'dataBody',           foreground: 'D07868' }, // data block after colon
  { token: 'punctuation',        foreground: '79987F' }, // declaration colons
  { token: 'reference',          foreground: 'A59582' }, // paths, aliases, …
  { token: 'declarationKeyword', foreground: '80807A' }, // spec, data, with, rule, repo, uses, meta
];

/**
 * Semantic token colours (Lemma dark theme).
 * Monaco uses dot notation for modifier-qualified rules (type.modifier).
 */
export const LEMMA_SEMANTIC_COLORS = {
  'namespace':          '#BD8EBB', // repo qualifier
  'class':              '#5DBDAA', // spec name
  'property':           '#A3B5DF', // data field path
  'function':           '#A3B5DF', // rule name
  'string':             '#D07868', // same hex as dataBody
  'comment':            '#595945',
  'keyword':            '#6BA0C2', // type + math keywords + constraint words
  'operator':           '#80807A',
  'controlKeyword':     '#726B83',
  'dataBody':           '#D07868', // data after colon
  'punctuation':        '#79987F', // data/rule colons
  'reference':          '#A59582',
  'declarationKeyword': '#80807A', // spec, data, with, rule, repo, uses, meta
};

/**
 * Complete Monaco theme built on the Lemma brand.
 * Pass directly to monaco.editor.defineTheme:
 *
 *   monaco.editor.defineTheme('lemma-dark', LEMMA_MONACO_THEME);
 *   monaco.editor.setTheme('lemma-dark');
 *
 * The editor background is set to transparent so the obsidian stone
 * surface from the page shows through unbroken (matching .lemma-editor CSS).
 * Set colors['editor.background'] to '#0B0B09' if you need an opaque editor.
 */
export const LEMMA_MONACO_THEME = {
  base: 'vs-dark',
  inherit: true,
  rules: LEMMA_MONACO_RULES,
  colors: {
    // Background transparent — stone surface shows through.
    // If you need opaque, override with '#0B0B09'.
    'editor.background':                '#0B0B0900',
    'editor.foreground':                '#EFE4C8', // soft ivory
    'editorLineNumber.foreground':      '#4A4A42',
    'editorLineNumber.activeForeground': '#8A8A78',
    'editorCursor.foreground':          '#C2B295', // sandstone
    'editor.selectionBackground':       '#C2B29530', // sandstone at ~19%
    'editor.lineHighlightBackground':   '#27272410', // offset, very subtle
    'editorIndentGuide.background1':    '#27272460',
    'editorIndentGuide.activeBackground1': '#C2B29550',
    'editorBracketMatch.background':    '#C2B29530',
    'editorBracketMatch.border':        '#C2B295',
    'scrollbarSlider.background':       '#C1C2BD20',
    'scrollbarSlider.hoverBackground':  '#C1C2BD40',
    'scrollbarSlider.activeBackground': '#C1C2BD60',
  },
};

export function registerLemmaLanguage(monaco) {
  monaco.languages.register({ id: 'lemma' });

  monaco.languages.setLanguageConfiguration('lemma', {
    comments: { blockComment: ['"""', '"""'] },
    // brackets: [['(', ')'], ['[', ']']],
    autoClosingPairs: [
      { open: '(', close: ')' },
      { open: '[', close: ']' },
      { open: '"', close: '"' },
      { open: '"""', close: '"""' },
    ],
    surroundingPairs: [
      { open: '(', close: ')' },
      { open: '[', close: ']' },
      { open: '"', close: '"' },
      { open: '"""', close: '"""' },
    ],
  });
}

/**
 * Register LSP-backed providers for semantic tokens and formatting.
 * Call after the LspClient has been initialized and didOpen sent.
 * URI for each request is derived from model.uri (multi-file support).
 *
 * @param {object} monaco     The monaco-editor API object
 * @param {object} lspClient  An initialized LspClient instance
 */
export function registerLspProviders(monaco, lspClient) {
  monaco.languages.registerDocumentSemanticTokensProvider('lemma', {
    getLegend() {
      return { tokenTypes: SEMANTIC_TOKEN_TYPES, tokenModifiers: SEMANTIC_TOKEN_MODIFIERS };
    },
    async provideDocumentSemanticTokens(model) {
      const uri = model.uri.toString();
      const result = await lspClient.semanticTokensFull(uri);
      if (!result || !result.data) return null;
      return { data: new Uint32Array(result.data) };
    },
    releaseDocumentSemanticTokens() {},
  });

  monaco.languages.registerCompletionItemProvider('lemma', {
    triggerCharacters: ['.', '>', ':', ' '],
    async provideCompletionItems(model, position) {
      const uri = model.uri.toString();
      const result = await lspClient.completion(
        uri,
        position.lineNumber - 1,
        position.column - 1,
      );
      const items = Array.isArray(result) ? result : (result && result.items) ? result.items : [];
      return {
        suggestions: items.map(function (item) {
          const range = item.textEdit && item.textEdit.range
            ? new monaco.Range(
                item.textEdit.range.start.line + 1,
                item.textEdit.range.start.character + 1,
                item.textEdit.range.end.line + 1,
                item.textEdit.range.end.character + 1,
              )
            : new monaco.Range(
                position.lineNumber,
                position.column,
                position.lineNumber,
                position.column,
              );
          const insert = (item.textEdit && item.textEdit.newText) || item.insertText || item.label;
          return {
            label: item.label,
            kind: monaco.languages.CompletionItemKind.Keyword,
            insertText: insert,
            detail: item.detail,
            documentation: item.documentation && item.documentation.value
              ? item.documentation.value
              : item.documentation,
            range: range,
          };
        }),
      };
    },
  });

  monaco.languages.registerDocumentFormattingEditProvider('lemma', {
    async provideDocumentFormattingEdits(model) {
      const uri = model.uri.toString();
      const edits = await lspClient.formatting(
        uri,
        model.getOptions().tabSize,
        model.getOptions().insertSpaces,
      );
      if (!Array.isArray(edits)) return [];
      return edits.map(function (edit) {
        return {
          range: new monaco.Range(
            edit.range.start.line + 1, edit.range.start.character + 1,
            edit.range.end.line + 1, edit.range.end.character + 1,
          ),
          text: edit.newText,
        };
      });
    },
  });
}
