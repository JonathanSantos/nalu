// Extensão da Nalu para o VS Code: lint com `nalu check --stdin <arquivo> --json [--explain]` no
// conteúdo do editor (sem precisar salvar), ⚡ no fim das linhas que podem falhar sem `catch`, tipos no
// hover, completar, ajuda de assinatura e ir para a definição (`nalu ide`), formatação (`nalu fmt`),
// integração com o GitHub Copilot (copilot.js) e comandos para rodar/gerar. Sem dependências: só a API do VS Code (e o check-json.js ao lado).
'use strict'
const vscode = require('vscode')
const { execFile } = require('child_process')
const path = require('path')
const fs = require('fs')
const os = require('os')
const saida = require('./check-json')
const copilot = require('./copilot')

let diagnostics
let output
let marcaFalha
let avisouBinario = false
let avisouResposta = false
const timers = new Map() // uri → setTimeout do lint com atraso
const rodadas = new Map() // uri → número da última checagem (respostas velhas são descartadas)
const falhaveis = new Map() // uri → DecorationOptions[] da última checagem sem erros de compilação

function activate(context) {
  diagnostics = vscode.languages.createDiagnosticCollection('nalu')
  output = vscode.window.createOutputChannel('Nalu')
  marcaFalha = vscode.window.createTextEditorDecorationType({
    after: { contentText: '⚡', margin: '0 0 0 1em', color: new vscode.ThemeColor('editorCodeLens.foreground') },
    rangeBehavior: vscode.DecorationRangeBehavior.OpenOpen,
  })
  context.subscriptions.push(diagnostics, output, marcaFalha)

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => lint(doc, 'open')),
    vscode.workspace.onDidChangeTextDocument((ev) => {
      if (ev.contentChanges.length > 0) lint(ev.document, 'type')
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => lint(doc, 'save')),
    vscode.workspace.onDidCloseTextDocument(esquecer),
    vscode.window.onDidChangeVisibleTextEditors((editores) => editores.forEach(marcar)),
    vscode.workspace.onDidChangeConfiguration(mudouConfiguracao),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      avisouBinario = false
    }),
    vscode.commands.registerCommand('nalu.check', () => comAtivo((doc) => checar(doc))),
    vscode.commands.registerCommand('nalu.run', () => comAtivo((doc) => noTerminal(doc, 'run'))),
    vscode.commands.registerCommand('nalu.build', () => comAtivo((doc) => noTerminal(doc, 'build'))),
  )

  const seletor = [
    { language: 'nalu', scheme: 'file' },
    { language: 'nalu', scheme: 'untitled' },
  ]
  context.subscriptions.push(
    vscode.languages.registerHoverProvider(seletor, { provideHover }),
    vscode.languages.registerCompletionItemProvider(seletor, { provideCompletionItems }, '.'),
    vscode.languages.registerSignatureHelpProvider(seletor, { provideSignatureHelp }, { triggerCharacters: ['(', ','], retriggerCharacters: [','] }),
    vscode.languages.registerDefinitionProvider(seletor, { provideDefinition }),
    vscode.languages.registerDocumentFormattingEditProvider(seletor, { provideDocumentFormattingEdits }),
  )

  // GitHub Copilot: ferramentas do modo agente, contexto das sugestões inline e instruções (copilot.js)
  copilot.ativar(context, { nalu, nomeDoArquivo, output, config })
  for (const doc of vscode.workspace.textDocuments) lint(doc, 'open')
}

function deactivate() {
  for (const t of timers.values()) clearTimeout(t)
  timers.clear()
}

function config(doc) {
  return vscode.workspace.getConfiguration('nalu', doc ? doc.uri : undefined)
}

/** Só arquivos .nalu de verdade (ou sem título): não os de `git:`, diffs etc. */
function ehNalu(doc) {
  return doc.languageId === 'nalu' && (doc.uri.scheme === 'file' || doc.uri.scheme === 'untitled')
}

function comAtivo(f) {
  const editor = vscode.window.activeTextEditor
  if (editor && ehNalu(editor.document)) return f(editor.document)
  vscode.window.showInformationMessage('Open a .nalu file first.')
}

function existe(p) {
  try {
    return fs.existsSync(p)
  } catch (e) {
    return false
  }
}

/**
 * Onde está o binário: a configuração `nalu.path` (se mudou do padrão), senão o `target/release/nalu`
 * ou `target/debug/nalu` de uma pasta aberta (o repositório da Nalu), senão o `nalu` do PATH.
 */
function binario() {
  let cfg = String(config().get('path') || '').trim()
  if (cfg && cfg !== 'nalu') {
    if (cfg === '~' || cfg.startsWith('~/')) cfg = path.join(os.homedir(), cfg.slice(1))
    const pasta = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath
    if (pasta && !path.isAbsolute(cfg) && /[\\/]/.test(cfg)) cfg = path.join(pasta, cfg)
    return cfg
  }
  const exe = process.platform === 'win32' ? 'nalu.exe' : 'nalu'
  for (const f of vscode.workspace.workspaceFolders || []) {
    for (const sub of ['target/release', 'target/debug']) {
      const p = path.join(f.uri.fsPath, sub, exe)
      if (existe(p)) return p
    }
  }
  return 'nalu'
}

/** Roda o `nalu` com `args`, mandando `stdin`. Resolve com { code, stdout, stderr }, ou null se nem rodou. */
function nalu(args, cwd, stdin) {
  const bin = binario()
  return new Promise((resolve) => {
    let child
    const opcoes = { cwd: cwd && existe(cwd) ? cwd : undefined, maxBuffer: 16 * 1024 * 1024, timeout: 20000, windowsHide: true }
    try {
      child = execFile(bin, args, opcoes, (err, stdout, stderr) => {
        // Erro ao criar o processo (ENOENT, EACCES…): o binário não está lá ou não executa.
        if (err && typeof err.code === 'string' && String(err.syscall || '').startsWith('spawn')) {
          semBinario(bin, err)
          return resolve(null)
        }
        const code = !err ? 0 : typeof err.code === 'number' ? err.code : 1
        resolve({ code, stdout: stdout || '', stderr: stderr || '' })
      })
    } catch (err) {
      semBinario(bin, err)
      return resolve(null)
    }
    if (stdin !== undefined && child.stdin) {
      child.stdin.on('error', () => {})
      child.stdin.end(stdin)
    }
  })
}

/** Binário ausente: uma notificação só (até `nalu.path` mudar), sem repetir a cada tecla. */
function semBinario(bin, err) {
  output.appendLine(`could not run \`${bin}\`: ${err.message}`)
  if (avisouBinario) return
  avisouBinario = true
  const abrir = 'Open settings'
  vscode.window
    .showErrorMessage(
      `Nalu: could not run \`${bin}\`. Put \`nalu\` on your PATH, build the Nalu repository with \`cargo build --release\` (the extension finds target/release/nalu in the open folder), or point the \`nalu.path\` setting at it.`,
      abrir,
    )
    .then((escolha) => {
      if (escolha === abrir) vscode.commands.executeCommand('workbench.action.openSettings', 'nalu.path')
    })
}

function respostaInesperada(args, r, err) {
  output.appendLine(`nalu ${args.join(' ')}: unexpected output (${err.message}; exit code ${r.code})`)
  if (r.stdout.trim()) output.appendLine(r.stdout)
  if (r.stderr.trim()) output.appendLine(r.stderr)
  if (avisouResposta) return
  avisouResposta = true
  vscode.window.showWarningMessage('Nalu: `nalu check` returned something the extension does not understand (an older binary?). See the "Nalu" output panel.')
}

function lint(doc, motivo) {
  if (!ehNalu(doc)) return
  const modo = config(doc).get('lint')
  if (modo === 'off') return
  if (motivo === 'type' && modo !== 'onType') return
  agendar(doc, motivo === 'type' ? 300 : 0)
}

function agendar(doc, atraso) {
  const key = doc.uri.toString()
  clearTimeout(timers.get(key))
  timers.set(
    key,
    setTimeout(() => {
      timers.delete(key)
      checar(doc)
    }, atraso),
  )
}

/** O nome que o `check` usa para mensagens e para resolver `embed`: o arquivo, ou um nome na pasta aberta. */
function nomeDoArquivo(doc) {
  if (doc.uri.scheme === 'file') return doc.uri.fsPath
  const pasta = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || os.tmpdir()
  return path.join(pasta, `${path.basename(doc.uri.path) || 'sem-titulo'}.nalu`)
}

/** `nalu check --stdin <arquivo> --json [--explain]` com o conteúdo do editor, sem precisar salvar. */
async function checar(doc) {
  if (doc.isClosed) return
  const key = doc.uri.toString()
  const rodada = (rodadas.get(key) || 0) + 1
  rodadas.set(key, rodada)
  const explicar = config(doc).get('explain') !== false
  const arquivo = nomeDoArquivo(doc)
  const args = ['check', '--stdin', arquivo, '--json']
  if (explicar) args.push('--explain')
  const r = await nalu(args, path.dirname(arquivo), doc.getText())
  if (!r || doc.isClosed || rodadas.get(key) !== rodada) return
  let res
  try {
    res = saida.lerSaida(r.stdout)
  } catch (err) {
    respostaInesperada(args, r, err)
    return
  }
  diagnostics.set(doc.uri, res.diagnosticos.map((d) => diagnostico(doc, d)))
  if (!explicar) falhaveis.delete(key)
  else if (res.falhaveis) falhaveis.set(key, decoracoes(doc, res.falhaveis))
  // Com erros de compilação o `--explain` não traz `fallible`: ficam as marcas da última checagem boa.
  for (const editor of vscode.window.visibleTextEditors) {
    if (editor.document === doc) marcar(editor)
  }
}

/** Um item do JSON do `check` vira um Diagnostic que sublinha a palavra na posição dada. */
function diagnostico(doc, d) {
  const linha = Math.min(Math.max(0, d.line - 1), Math.max(0, doc.lineCount - 1))
  const { inicio, fim } = saida.faixaNaLinha(doc.lineAt(linha).text, d.column)
  const severidade = d.severity === 'warning' ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Error
  const diag = new vscode.Diagnostic(new vscode.Range(linha, inicio, linha, fim), saida.mensagem(d), severidade)
  diag.source = 'nalu'
  if (d.code) diag.code = d.code
  // Variável ou `param` não usado: esmaecido, como nas outras linguagens.
  if (d.code === 'W001' || d.code === 'W002') diag.tags = [vscode.DiagnosticTag.Unnecessary]
  return diag
}

/** Os itens de `fallible` viram um ⚡ no fim de cada linha; o hover cobre a linha a partir da operação. */
function decoracoes(doc, itens) {
  return saida
    .agruparFalhaveis(itens)
    .filter((g) => g.linha < doc.lineCount)
    .map((g) => {
      const texto = doc.lineAt(g.linha).text
      const inicio = Math.min(saida.colunaUtf16(texto, g.coluna), texto.length)
      return {
        range: new vscode.Range(g.linha, inicio, g.linha, texto.length),
        hoverMessage: new vscode.MarkdownString(saida.textoHover(g.itens)),
      }
    })
}

// ---------------------------------------------------------------------------------------------
// `nalu ide`: hover, completar, assinatura e definição
// ---------------------------------------------------------------------------------------------

/** `nalu ide <consulta> <arquivo> --stdin --at L:C` com o texto do editor. JSON (ou null) da resposta. */
async function consultar(consulta, doc, pos, token) {
  const arquivo = nomeDoArquivo(doc)
  const at = saida.posicaoNalu(doc.lineAt(pos.line).text, pos.line, pos.character)
  const args = ['ide', consulta, arquivo, '--stdin', '--at', at]
  const r = await nalu(args, path.dirname(arquivo), doc.getText())
  if (!r || (token && token.isCancellationRequested)) return null
  if (r.code !== 0) {
    output.appendLine(`nalu ${args.join(' ')}: exit code ${r.code}\n${r.stderr}`)
    return null
  }
  try {
    return JSON.parse(r.stdout)
  } catch (err) {
    respostaInesperada(args, r, err)
    return null
  }
}

function faixa(doc, r) {
  const f = saida.faixaIde(r, (n) => (n < doc.lineCount ? doc.lineAt(n).text : ''))
  return new vscode.Range(f.linha, f.inicio, f.linhaFim, f.fim)
}

async function provideHover(doc, pos, token) {
  const r = await consultar('hover', doc, pos, token)
  if (!r || !r.markdown) return null
  return new vscode.Hover(new vscode.MarkdownString(r.markdown), faixa(doc, r))
}

const TIPOS = {
  variable: 'Variable',
  function: 'Function',
  method: 'Method',
  field: 'Field',
  property: 'Property',
  module: 'Module',
  struct: 'Struct',
  enum: 'Enum',
  case: 'EnumMember',
  keyword: 'Keyword',
  snippet: 'Snippet',
  type: 'TypeParameter',
  constant: 'Constant',
}

async function provideCompletionItems(doc, pos, token) {
  const r = await consultar('complete', doc, pos, token)
  if (!r || !Array.isArray(r.items)) return null
  const inicio = faixa(doc, { line: r.line, column: r.column }).start
  const troca = new vscode.Range(inicio.line === pos.line ? inicio : pos, pos)
  return r.items.map((it, i) => {
    const item = new vscode.CompletionItem(it.label, vscode.CompletionItemKind[TIPOS[it.kind] || 'Text'])
    if (it.detail) item.detail = it.detail
    if (it.doc) item.documentation = new vscode.MarkdownString(it.doc)
    if (it.insert) item.insertText = saida.ehSnippet(it.insert) ? new vscode.SnippetString(it.insert) : it.insert
    item.range = troca
    // a ordem do compilador: nomes do escopo, funções, tipos, módulos, palavras-chave
    item.sortText = String(i).padStart(4, '0')
    if (it.kind === 'method' || it.kind === 'function') item.command = { command: 'editor.action.triggerParameterHints', title: '' }
    return item
  })
}

async function provideSignatureHelp(doc, pos, token) {
  const r = await consultar('signature', doc, pos, token)
  if (!r || !r.label) return null
  const info = new vscode.SignatureInformation(r.label, r.doc ? new vscode.MarkdownString(r.doc) : undefined)
  info.parameters = (r.params || []).map(([a, b]) => new vscode.ParameterInformation([saida.colunaUtf16(r.label, a), saida.colunaUtf16(r.label, b)]))
  const ajuda = new vscode.SignatureHelp()
  ajuda.signatures = [info]
  ajuda.activeSignature = 0
  ajuda.activeParameter = Math.min(r.active || 0, Math.max(0, info.parameters.length - 1))
  return ajuda
}

async function provideDefinition(doc, pos, token) {
  const r = await consultar('definition', doc, pos, token)
  if (!r) return null
  const mesmo = !r.file || r.file === nomeDoArquivo(doc)
  if (mesmo) return new vscode.Location(doc.uri, faixa(doc, r))
  // outro arquivo do pacote (`x.nalu` ao lado de `x_test.nalu`)
  const outro = await vscode.workspace.openTextDocument(vscode.Uri.file(path.resolve(path.dirname(nomeDoArquivo(doc)), r.file)))
  return new vscode.Location(outro.uri, faixa(outro, r))
}

// ---------------------------------------------------------------------------------------------
// `nalu fmt`
// ---------------------------------------------------------------------------------------------

async function provideDocumentFormattingEdits(doc, _opcoes, token) {
  const arquivo = nomeDoArquivo(doc)
  const texto = doc.getText()
  const r = await nalu(['fmt', '--stdin', arquivo], path.dirname(arquivo), texto)
  if (!r || token.isCancellationRequested) return []
  if (r.code !== 0) {
    output.appendLine(`nalu fmt: exit code ${r.code}\n${r.stderr}`)
    // 1: erros de sintaxe; 2: uso errado, ou seja, um binário antigo, sem `fmt`
    const msg = r.code === 1 ? 'Nalu: fix the syntax errors to format this file' : 'Nalu: this `nalu` binary cannot format; rebuild it (see the "Nalu" output panel)'
    vscode.window.setStatusBarMessage(msg, 5000)
    return []
  }
  if (r.stdout === texto) return []
  const tudo = new vscode.Range(doc.positionAt(0), doc.positionAt(texto.length))
  return [vscode.TextEdit.replace(tudo, r.stdout)]
}

function marcar(editor) {
  if (!editor || !ehNalu(editor.document)) return
  const ligado = config(editor.document).get('explain') !== false
  editor.setDecorations(marcaFalha, ligado ? falhaveis.get(editor.document.uri.toString()) || [] : [])
}

function esquecer(doc) {
  const key = doc.uri.toString()
  clearTimeout(timers.get(key))
  timers.delete(key)
  rodadas.delete(key)
  falhaveis.delete(key)
  diagnostics.delete(doc.uri)
}

function mudouConfiguracao(ev) {
  if (!ev.affectsConfiguration('nalu')) return
  if (ev.affectsConfiguration('nalu.path')) avisouBinario = false
  avisouResposta = false
  if (ev.affectsConfiguration('nalu.explain')) falhaveis.clear()
  for (const doc of vscode.workspace.textDocuments) {
    if (!ehNalu(doc)) continue
    if (config(doc).get('lint') === 'off') {
      diagnostics.delete(doc.uri)
      falhaveis.delete(doc.uri.toString())
    } else {
      agendar(doc, 0)
    }
  }
  vscode.window.visibleTextEditors.forEach(marcar)
}

async function noTerminal(doc, sub) {
  if (doc.isUntitled) {
    vscode.window.showInformationMessage(`Save the file before running \`nalu ${sub}\`.`)
    return
  }
  if (doc.isDirty && !(await doc.save())) return
  const term = vscode.window.terminals.find((t) => t.name === 'nalu') || vscode.window.createTerminal('nalu')
  term.show()
  term.sendText(`${binario()} ${sub} ${aspas(doc.uri.fsPath)}`)
}

function aspas(s) {
  return /[\s"'$`\\]/.test(s) ? `'${s.replace(/'/g, "'\\''")}'` : s
}

module.exports = { activate, deactivate }
