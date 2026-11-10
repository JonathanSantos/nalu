// Integração com o GitHub Copilot (D29). Nada aqui conhece a linguagem: tudo vem do compilador.
// - Ferramentas do modo agente (`nalu_check`, `nalu_docs`, `nalu_format`): o agente confere o código que
//   escreveu com o compilador de verdade, consulta a stdlib e formata.
// - Provedor de contexto das sugestões inline: a cada sugestão, as regras da linguagem, os nomes visíveis no
//   cursor e a parte da stdlib que importa ali (`nalu ide context`). API do Copilot sem documentação oficial
//   (os tipos estão em @github/copilot-language-server). Limite conferido no Copilot do VS Code 1.144: a API
//   pública registra o provedor só no motor clássico de completions (não no modelo de "next edit", que hoje
//   gera as sugestões inline) e esse motor só consulta ids liberados (os do próprio Copilot, experimentos do
//   servidor ou a configuração `github.copilot.advanced` → `contextProviders`). O painel "Nalu" mostra o
//   primeiro pedido e as estatísticas de uso, se um dia vierem.
// - Instruções para o chat: `copilot/nalu.instructions.md`, contribuído no package.json; o comando
//   `nalu.addCopilotInstructions` copia o arquivo para o projeto (para o time e para o Copilot no GitHub).
'use strict'
const vscode = require('vscode')
const path = require('path')
const fs = require('fs')
const { colunaCodePoints } = require('./check-json')

let deps // { nalu(args, cwd, stdin), nomeDoArquivo(doc), output, config(doc) }

function ativar(context, dependencias) {
  deps = dependencias
  registrarFerramentas(context)
  registrarContexto(context)
  context.subscriptions.push(vscode.commands.registerCommand('nalu.addCopilotInstructions', () => adicionarInstrucoes(context)))
}

// ---------------------------------------------------------------------------------------------
// Ferramentas do modo agente
// ---------------------------------------------------------------------------------------------

const FERRAMENTAS = {
  nalu_check: { mensagem: 'Checking Nalu code with the compiler', invocar: checar },
  nalu_docs: { mensagem: 'Reading the Nalu documentation', invocar: documentar },
  nalu_format: { mensagem: 'Formatting Nalu code', invocar: formatar },
}

function registrarFerramentas(context) {
  if (!vscode.lm || typeof vscode.lm.registerTool !== 'function') return
  for (const [nome, f] of Object.entries(FERRAMENTAS)) {
    context.subscriptions.push(
      vscode.lm.registerTool(nome, {
        prepareInvocation: () => ({ invocationMessage: f.mensagem }),
        invoke: async (opcoes, token) => {
          const texto = await f.invocar((opcoes && opcoes.input) || {}, token)
          return new vscode.LanguageModelToolResult([new vscode.LanguageModelTextPart(texto)])
        },
      }),
    )
  }
}

function pastaDoProjeto() {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || process.cwd()
}

/**
 * O código a conferir: `code` (texto) ou `path` (arquivo; com o texto do editor se estiver aberto); sem
 * nenhum dos dois, o arquivo .nalu ativo. Devolve { arquivo, texto }.
 */
function alvo(input) {
  const caminho = typeof input.path === 'string' && input.path.trim() ? path.resolve(pastaDoProjeto(), input.path.trim()) : null
  if (typeof input.code === 'string') {
    return { arquivo: caminho || path.join(pastaDoProjeto(), 'snippet.nalu'), texto: input.code }
  }
  if (caminho) {
    const aberto = vscode.workspace.textDocuments.find((d) => d.uri.scheme === 'file' && d.uri.fsPath === caminho)
    if (aberto) return { arquivo: caminho, texto: aberto.getText() }
    try {
      return { arquivo: caminho, texto: fs.readFileSync(caminho, 'utf8') }
    } catch (err) {
      throw new Error(`Could not read ${caminho}: ${err.message}`)
    }
  }
  const ed = vscode.window.activeTextEditor
  if (ed && ed.document.languageId === 'nalu') return { arquivo: deps.nomeDoArquivo(ed.document), texto: ed.document.getText() }
  throw new Error('Pass `code` (the Nalu source) or `path` (a .nalu file).')
}

async function rodar(args, cwd, stdin) {
  const r = await deps.nalu(args, cwd, stdin)
  if (!r) throw new Error('The nalu compiler could not be started. Install it, or set the `nalu.path` setting to the binary.')
  return r
}

async function checar(input) {
  const { arquivo, texto } = alvo(input)
  const r = await rodar(['check', '--stdin', arquivo, '--explain'], path.dirname(arquivo), texto)
  const diagnosticos = r.stderr.trim()
  const falhaveis = r.stdout.trim()
  const partes = [r.code === 0 ? 'No compile errors.' : 'The code has compile errors. Fix every error, then check again.']
  if (diagnosticos) partes.push(diagnosticos)
  if (r.code === 0 && falhaveis) {
    partes.push(`Lines that can fail without \`catch\` (fine when the failure should stop the script or make the function fail):\n${falhaveis}`)
  }
  return partes.join('\n\n')
}

async function documentar(input) {
  const nomes = Array.isArray(input.names) ? input.names.map(String).filter((n) => n.trim()) : typeof input.names === 'string' ? input.names.split(/[\s,]+/).filter(Boolean) : []
  const r = await rodar(['doc', ...nomes], pastaDoProjeto())
  return [r.stdout.trim(), r.stderr.trim()].filter(Boolean).join('\n\n') || 'No documentation found.'
}

async function formatar(input) {
  const { arquivo, texto } = alvo(input)
  const r = await rodar(['fmt', '--stdin', arquivo], path.dirname(arquivo), texto)
  if (r.code !== 0) return `The code has syntax errors, so it was not formatted:\n\n${r.stderr.trim()}`
  return r.stdout === texto ? 'Already formatted.' : `Formatted code:\n\n\`\`\`nalu\n${r.stdout}\`\`\``
}

// ---------------------------------------------------------------------------------------------
// Contexto para as sugestões inline
// ---------------------------------------------------------------------------------------------

const registrados = new Set() // ids das extensões do Copilot que já receberam o provedor
let ultimo = new Map() // uri → { chave, itens } da última resposta
let tracos = [] // as regras fixas, guardadas para quando o tempo acabar antes do compilador
const uso = { full: 0, partial: 0, none: 0, outros: 0, pedidos: 0, chamadas: 0 }

async function apiDoCopilot(id) {
  const ext = vscode.extensions.getExtension(id)
  if (!ext) return undefined
  let exp
  try {
    exp = await ext.activate()
  } catch (err) {
    return undefined
  }
  // `github.copilot` exporta getContextProviderAPI direto; `github.copilot-chat`, por getAPI(1)
  const api = id === 'github.copilot-chat' && exp && typeof exp.getAPI === 'function' ? exp.getAPI(1) : exp
  if (!api || typeof api.getContextProviderAPI !== 'function') return undefined
  try {
    return await api.getContextProviderAPI('v1')
  } catch (err) {
    return undefined
  }
}

async function registrarContexto(context) {
  const tentar = async () => {
    if (deps.config().get('copilotContext') === false) return
    for (const id of ['github.copilot', 'github.copilot-chat']) {
      if (registrados.has(id)) continue
      const api = await apiDoCopilot(id)
      if (!api || typeof api.registerContextProvider !== 'function') continue
      registrados.add(id)
      context.subscriptions.push(api.registerContextProvider(provedor))
      deps.output.appendLine(`Copilot: Nalu context provider registered with ${id}`)
    }
  }
  await tentar()
  // o Copilot pode ser instalado ou ativado depois
  if (vscode.extensions.onDidChange) context.subscriptions.push(vscode.extensions.onDidChange(() => tentar()))
}

const provedor = {
  id: 'nalu',
  selector: [{ language: 'nalu' }],
  resolver: {
    resolve: (pedido, token) => resolver(pedido, token),
    // o tempo acabou: as regras fixas e o último contexto deste arquivo
    resolveOnTimeout: (pedido) => {
      const uri = pedido && pedido.documentContext && String(pedido.documentContext.uri)
      return (uri && ultimo.get(uri) && ultimo.get(uri).itens) || tracos
    },
  },
}

/** `timeoutEnd` em milissegundos (a API não diz a unidade: valores pequenos são segundos). */
function prazo(pedido, agora) {
  const t = Number(pedido && pedido.timeoutEnd)
  if (Number.isFinite(t) && t > 0) return t > 1e12 ? t : t * 1000
  const b = Number(pedido && pedido.timeBudget)
  return agora + (Number.isFinite(b) && b > 0 ? b : 150)
}

function contarUso(stats) {
  if (!stats) return
  uso.pedidos++
  if (stats.usage === 'full') uso.full++
  else if (stats.usage === 'partial' || stats.usage === 'partial_content_excluded') uso.partial++
  else if (stats.usage === 'none' || stats.usage === 'none_content_excluded') uso.none++
  else uso.outros++
  if (uso.pedidos % 25 === 0) {
    deps.output.appendLine(`Copilot used the Nalu context in the last ${uso.pedidos} suggestions: full ${uso.full}, partial ${uso.partial}, none ${uso.none}, other ${uso.outros}`)
  }
}

/** A resposta do `nalu ide context` vira itens do Copilot: traços (regras) e trechos de código. */
function itensDoContexto(r, uri) {
  const t = (r.traits || []).map((x) => ({ id: `nalu-${x.name.toLowerCase()}`, name: x.name, value: x.value, importance: 90 }))
  const s = (r.snippets || []).map((x) => ({ id: `nalu-${x.name}`, uri, value: x.code, importance: Math.max(0, Math.min(100, x.importance | 0)) }))
  return { tracos: t, itens: t.concat(s) }
}

async function resolver(pedido, token) {
  if (uso.chamadas++ === 0) deps.output.appendLine('Copilot: first request for Nalu context in this session')
  contarUso(pedido && pedido.previousUsageStatistics)
  const ctx = pedido && pedido.documentContext
  if (!ctx) return tracos
  const uri = String(ctx.uri)
  const doc = vscode.workspace.textDocuments.find((d) => d.uri.toString() === uri)
  if (!doc || doc.languageId !== 'nalu') return []
  const pos = ctx.position || doc.positionAt(ctx.offset || 0)
  const chave = `${doc.version}:${pos.line}`
  const guardado = ultimo.get(uri)
  if (guardado && guardado.chave === chave) return guardado.itens
  const agora = Date.now()
  const resta = prazo(pedido, agora) - agora - 5
  if (resta <= 0) return (guardado && guardado.itens) || tracos
  const arquivo = deps.nomeDoArquivo(doc)
  const at = `${pos.line + 1}:${colunaCodePoints(doc.lineAt(pos.line).text, pos.character) + 1}`
  const consulta = deps.nalu(['ide', 'context', arquivo, '--stdin', '--at', at], path.dirname(arquivo), doc.getText())
  const r = await Promise.race([consulta, new Promise((ok) => setTimeout(() => ok('tempo'), resta))])
  if (r === 'tempo' || !r || r.code !== 0 || (token && token.isCancellationRequested)) return (guardado && guardado.itens) || tracos
  let json
  try {
    json = JSON.parse(r.stdout)
  } catch (err) {
    return tracos
  }
  const { tracos: t, itens } = itensDoContexto(json, uri)
  tracos = t
  ultimo.set(uri, { chave, itens })
  if (ultimo.size > 20) ultimo = new Map([...ultimo].slice(-10))
  return itens
}

// ---------------------------------------------------------------------------------------------
// Instruções para o chat
// ---------------------------------------------------------------------------------------------

/** Copia `copilot/nalu.instructions.md` para `.github/instructions/` do projeto (pergunta antes de trocar). */
async function adicionarInstrucoes(context) {
  const pasta = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath
  if (!pasta) {
    vscode.window.showInformationMessage('Open a folder first: the instructions go into its .github/instructions folder.')
    return
  }
  const origem = path.join(context.extensionPath, 'copilot', 'nalu.instructions.md')
  const destino = path.join(pasta, '.github', 'instructions', 'nalu.instructions.md')
  const novo = fs.readFileSync(origem, 'utf8')
  if (fs.existsSync(destino)) {
    if (fs.readFileSync(destino, 'utf8') === novo) {
      vscode.window.showInformationMessage('The Nalu Copilot instructions in this folder are already up to date.')
      return
    }
    const trocar = 'Replace'
    const escolha = await vscode.window.showWarningMessage(`${path.relative(pasta, destino)} already exists and is different. Replace it with the version from the Nalu extension?`, { modal: true }, trocar)
    if (escolha !== trocar) return
  }
  fs.mkdirSync(path.dirname(destino), { recursive: true })
  fs.writeFileSync(destino, novo)
  const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(destino))
  await vscode.window.showTextDocument(doc)
  vscode.window.showInformationMessage(`Added ${path.relative(pasta, destino)}: Copilot chat and agents now follow the Nalu rules in this project.`)
}

module.exports = { ativar, prazo, itensDoContexto, provedor, resolver, uso }
