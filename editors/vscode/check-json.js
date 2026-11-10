// Leitura da saída de `nalu check --stdin <arquivo> --json [--explain]`, sem depender da API do
// VS Code: o extension.js usa estas funções e dá para testá-las com `node` puro.
'use strict'

/**
 * Interpreta o stdout do `nalu check --json`, nos dois formatos:
 * - uma lista de diagnósticos (sem `--explain`, ou com `--explain` quando há erros de compilação);
 * - `{ diagnostics, fallible, fallible_fns }` (com `--explain` e sem erros).
 * Devolve `{ diagnosticos, falhaveis, funcoesFalhaveis }`; `falhaveis` e `funcoesFalhaveis` são
 * `null` quando a saída não os traz. Lança um Error se o texto não for nenhum dos dois formatos.
 */
function lerSaida(stdout) {
  const dados = JSON.parse(stdout)
  if (Array.isArray(dados)) {
    return { diagnosticos: dados.filter(ehDiagnostico), falhaveis: null, funcoesFalhaveis: null }
  }
  if (dados && typeof dados === 'object' && Array.isArray(dados.diagnostics)) {
    return {
      diagnosticos: dados.diagnostics.filter(ehDiagnostico),
      falhaveis: Array.isArray(dados.fallible) ? dados.fallible.filter(ehPosicao) : null,
      funcoesFalhaveis: Array.isArray(dados.fallible_fns) ? dados.fallible_fns.filter((f) => typeof f === 'string') : null,
    }
  }
  throw new Error('unexpected output: neither a list of diagnostics nor an object with "diagnostics"')
}

function ehPosicao(x) {
  return !!x && typeof x === 'object' && Number.isInteger(x.line) && Number.isInteger(x.column)
}

function ehDiagnostico(x) {
  return ehPosicao(x) && typeof x.message === 'string'
}

/** Texto do diagnóstico: a mensagem e, se houver, a ajuda do compilador. */
function mensagem(d) {
  return d.help ? `${d.message}\nhelp: ${d.help}` : d.message
}

/**
 * O compilador conta colunas em code points; o VS Code, em unidades UTF-16. Converte uma coluna
 * 0-based da linha `texto` (só muda depois de caracteres fora do plano básico, como emoji).
 */
function colunaUtf16(texto, coluna) {
  let cp = 0
  let u = 0
  for (const ch of texto) {
    if (cp >= coluna) return u
    cp++
    u += ch.length
  }
  return u + Math.max(0, coluna - cp)
}

/** O inverso de `colunaUtf16`: quantos code points há antes da coluna `u` (UTF-16, 0-based) de `texto`. */
function colunaCodePoints(texto, u) {
  let cp = 0
  let i = 0
  for (const ch of texto) {
    if (i >= u) return cp
    i += ch.length
    cp++
  }
  return cp + Math.max(0, u - i)
}

/** `--at LINHA:COLUNA` do `nalu ide` (1-based, em code points) para uma posição do VS Code. */
function posicaoNalu(textoDaLinha, linha, caractere) {
  return `${linha + 1}:${colunaCodePoints(textoDaLinha, caractere) + 1}`
}

/**
 * Um trecho do `nalu ide` (`line`, `column`, `end_line`, `end_column`, 1-based em code points) em
 * posições 0-based do VS Code; `linhaTexto(n)` dá o texto da linha n.
 */
function faixaIde(r, linhaTexto) {
  const l1 = Math.max(0, r.line - 1)
  const l2 = Math.max(0, (r.end_line || r.line) - 1)
  return {
    linha: l1,
    inicio: colunaUtf16(linhaTexto(l1), Math.max(0, r.column - 1)),
    linhaFim: l2,
    fim: colunaUtf16(linhaTexto(l2), Math.max(0, (r.end_column || r.column) - 1)),
  }
}

/** O texto inserido por um item de completar é um snippet (`${1:x}`, `$0`)? */
function ehSnippet(texto) {
  return typeof texto === 'string' && /\$(\{\d|\d)/.test(texto)
}

// O que um diagnóstico sublinha: `{…}`, um nome (com `.`), um texto entre aspas ou a palavra toda.
const PALAVRA = /^(?:\{[^{}]*\}|[A-Za-z_][A-Za-z0-9_.]*|"(?:[^"\\]|\\.)*"|'[^']*'|\S+)/

/**
 * Faixa sublinhada por um diagnóstico na linha `texto`, a partir da coluna do compilador (1-based,
 * em code points). Devolve `{ inicio, fim }` 0-based em UTF-16, com pelo menos um caractere.
 */
function faixaNaLinha(texto, coluna) {
  const inicio = colunaUtf16(texto, Math.max(0, (coluna || 1) - 1))
  const m = texto.slice(inicio).match(PALAVRA)
  return { inicio, fim: inicio + Math.max(1, m ? m[0].length : 1) }
}

/**
 * Agrupa os itens de `fallible` pela linha onde começam (um ⚡ por linha). Devolve, em ordem de
 * linha, `[{ linha, coluna, itens: [{ what, effect }] }]`: `linha` 0-based e `coluna` 0-based em
 * code points (a menor da linha).
 */
function agruparFalhaveis(falhaveis) {
  const porLinha = new Map()
  for (const f of falhaveis) {
    const linha = Math.max(0, f.line - 1)
    const coluna = Math.max(0, f.column - 1)
    let g = porLinha.get(linha)
    if (!g) porLinha.set(linha, (g = { linha, coluna, itens: [] }))
    g.coluna = Math.min(g.coluna, coluna)
    g.itens.push({ what: String(f.what || 'this operation'), effect: String(f.effect || 'the failure propagates') })
  }
  return [...porLinha.values()].sort((a, b) => a.linha - b.linha)
}

/** Markdown do hover de um ⚡: uma linha "⚡ `x` can fail → efeito" por item, e como tratar. */
function textoHover(itens) {
  const linhas = itens.map((i) => `⚡ ${i.what} ${i.what === '`fail`' ? 'fails' : 'can fail'} → ${i.effect}`)
  return `${linhas.join('  \n')}\n\n_Without \`catch\`, the failure propagates. To handle it: \`catch value\`, \`catch fail "…"\`, \`catch ignore\` or \`catch { … }\`._`
}

module.exports = { lerSaida, mensagem, colunaUtf16, colunaCodePoints, posicaoNalu, faixaIde, ehSnippet, faixaNaLinha, agruparFalhaveis, textoHover }
