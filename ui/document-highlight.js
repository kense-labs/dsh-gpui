// TextMate tokenization for the GPUI file view. The renderer receives only ranges and styles.
const grammars = new Map([
  ['typescript', () => import('@shikijs/langs/typescript')],
  ['shellscript', () => import('@shikijs/langs/shellscript')],
  ['json', () => import('@shikijs/langs/json')],
  ['python', () => import('@shikijs/langs/python')],
  ['ruby', () => import('@shikijs/langs/ruby')],
  ['go', () => import('@shikijs/langs/go')],
  ['rust', () => import('@shikijs/langs/rust')],
  ['java', () => import('@shikijs/langs/java')],
  ['c', () => import('@shikijs/langs/c')],
  ['cpp', () => import('@shikijs/langs/cpp')],
  ['csharp', () => import('@shikijs/langs/csharp')],
  ['kotlin', () => import('@shikijs/langs/kotlin')],
  ['swift', () => import('@shikijs/langs/swift')],
  ['php', () => import('@shikijs/langs/php')],
  ['yaml', () => import('@shikijs/langs/yaml')],
  ['toml', () => import('@shikijs/langs/toml')],
  ['ini', () => import('@shikijs/langs/ini')],
  ['markdown', () => import('@shikijs/langs/markdown')],
  ['mdx', () => import('@shikijs/langs/mdx')],
  ['html', () => import('@shikijs/langs/html')],
  ['css', () => import('@shikijs/langs/css')],
  ['scss', () => import('@shikijs/langs/scss')],
  ['less', () => import('@shikijs/langs/less')],
  ['sql', () => import('@shikijs/langs/sql')],
  ['xml', () => import('@shikijs/langs/xml')],
  ['lua', () => import('@shikijs/langs/lua')],
  ['bat', () => import('@shikijs/langs/bat')],
  ['powershell', () => import('@shikijs/langs/powershell')],
  ['fish', () => import('@shikijs/langs/fish')],
  ['dotenv', () => import('@shikijs/langs/dotenv')],
  ['log', () => import('@shikijs/langs/log')],
  ['csv', () => import('@shikijs/langs/csv')],
  ['diff', () => import('@shikijs/langs/diff')],
  ['http', () => import('@shikijs/langs/http')],
  ['rst', () => import('@shikijs/langs/rst')],
  ['latex', () => import('@shikijs/langs/latex')],
  ['bibtex', () => import('@shikijs/langs/bibtex')],
  ['asciidoc', () => import('@shikijs/langs/asciidoc')],
  ['r', () => import('@shikijs/langs/r')],
  ['julia', () => import('@shikijs/langs/julia')],
  ['dart', () => import('@shikijs/langs/dart')],
  ['scala', () => import('@shikijs/langs/scala')],
  ['clojure', () => import('@shikijs/langs/clojure')],
  ['erlang', () => import('@shikijs/langs/erlang')],
  ['elixir', () => import('@shikijs/langs/elixir')],
  ['haskell', () => import('@shikijs/langs/haskell')],
  ['fsharp', () => import('@shikijs/langs/fsharp')],
  ['vb', () => import('@shikijs/langs/vb')],
  ['perl', () => import('@shikijs/langs/perl')],
  ['verilog', () => import('@shikijs/langs/verilog')],
  ['system-verilog', () => import('@shikijs/langs/system-verilog')],
  ['graphql', () => import('@shikijs/langs/graphql')],
  ['proto', () => import('@shikijs/langs/proto')],
  ['hcl', () => import('@shikijs/langs/hcl')],
  ['nix', () => import('@shikijs/langs/nix')],
  ['vue', () => import('@shikijs/langs/vue')],
  ['svelte', () => import('@shikijs/langs/svelte')],
  ['make', () => import('@shikijs/langs/make')],
  ['cmake', () => import('@shikijs/langs/cmake')],
  ['groovy', () => import('@shikijs/langs/groovy')],
]);
const pending = new Map();
let engine;
const bootSamples = new Map([
  ['typescript', 'const answer: number = 42'],
  ['shellscript', 'printf \'%s\\n\' "$HOME"'],
  ['json', '{"ready":true}'],
]);

function highlighter() {
  engine ??= Promise.all([
    import('@shikijs/core'),
    import('@shikijs/engine-javascript'),
    ...Array.from(bootSamples.keys(), language => grammars.get(language)()),
  ]).then(([{ createHighlighterCoreSync, createCssVariablesTheme }, regex, ...boot]) => {
    const instance = createHighlighterCoreSync({
      themes: [createCssVariablesTheme({ name: 'css-variables', variablePrefix: '--shiki-', fontStyle: true })],
      langs: boot.map(module => module.default),
      engine: regex.createJavaScriptRegexEngine({
        forgiving: true,
        regexConstructor: pattern => regex.defaultJavaScriptRegexConstructor(pattern, {
          lazyCompileLength: Number.POSITIVE_INFINITY,
        }),
      }),
    });
    for (const [lang, sample] of bootSamples) {
      instance.codeToTokens(sample, { lang, theme: 'css-variables', tokenizeTimeLimit: 0 });
    }
    return instance;
  });
  return engine;
}

function repairGoGrammar(definitions) {
  // Match Harness's workaround for exponential backtracking in the Go struct-field rule.
  return definitions.map(grammar => {
    if (grammar.name !== 'go') return grammar;
    const fields = grammar.repository.struct_variables_types_fields;
    const patterns = fields.patterns.map(rule => rule.match?.includes('[^"/`\\s]+;?')
      ? { ...rule, match: '(?:(?<=\\{)([^/\\`"]*[^\\s/\\`"])\\s*(?=\\}))' }
      : rule);
    return { ...grammar, repository: {
      ...grammar.repository, struct_variables_types_fields: { ...fields, patterns },
    } };
  });
}

function load(language) {
  const grammar = language === 'javascript' ? 'typescript' : language;
  if (!pending.has(grammar)) {
    const getGrammar = grammars.get(grammar);
    if (!getGrammar) return undefined;
    pending.set(grammar, Promise.all([highlighter(), getGrammar()]).then(([instance, module]) => {
      if (!instance.getLoadedLanguages().includes(grammar)) {
        instance.loadLanguageSync(grammar === 'go' ? repairGoGrammar(module.default) : module.default);
      }
      return instance;
    }));
  }
  return pending.get(grammar);
}

const encoder = new TextEncoder();
function tokenLine(tokens) {
  let offset = 0;
  const spans = [];
  for (const token of tokens) {
    // Shiki uses UTF-16 indices. GPUI styles use UTF-8 byte ranges.
    const len = /^[\x00-\x7f]*$/.test(token.content)
      ? token.content.length
      : encoder.encode(token.content).length;
    const color = token.color?.match(/^var\(--shiki-token-([\w-]+)\)$/)?.[1];
    const style = Math.max(0, token.fontStyle ?? 0);
    if (len && (color || style)) spans.push({ start: offset, end: offset + len, color, style });
    offset += len;
  }
  return spans;
}

export class HighlightSession {
  #prefix = '';
  #lines = [];
  #state;

  async highlight(source, language) {
    const instance = await load(language);
    if (!instance) return [];
    if (!source.startsWith(this.#prefix)) {
      this.#prefix = '';
      this.#lines = [];
      this.#state = undefined;
    }
    const tokenize = text => instance.codeToTokensBase(text, {
      lang: language === 'javascript' ? 'typescript' : language,
      theme: 'css-variables',
      tokenizeMaxLineLength: 1000,
      grammarState: this.#state,
    });

    const rest = source.slice(this.#prefix.length);
    const lastBreak = rest.lastIndexOf('\n');
    if (lastBreak >= 0) {
      const end = rest[lastBreak - 1] === '\r' ? lastBreak - 1 : lastBreak;
      const complete = tokenize(rest.slice(0, end));
      for (const line of complete) this.#lines.push(tokenLine(line));
      this.#state = instance.getLastGrammarState(complete);
      this.#prefix = source.slice(0, this.#prefix.length + lastBreak + 1);
    }
    return [...this.#lines, ...tokenize(rest.slice(lastBreak + 1)).map(tokenLine)];
  }
}
