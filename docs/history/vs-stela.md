# Nalu × Stela

A Stela (pasta vizinha `mis/`) é o projeto que a Nalu disputa. As duas têm o **mesmo intuito**: um fonte tipado que vira Bash (Linux/macOS) e PowerShell (Windows), sem runtime no destino. Este documento registra o que a Nalu copia sem vergonha, o que muda e por quê, e como vamos medir quem ganha.

## O que a Stela acertou (e a Nalu copia)

- `$ cmd` como token léxico, `{ }` como **um** argumento, `$( )` captura, `valor | $ cmd` para stdin.
- `param` → `--help` gerado, validação, kebab-case, `///` como descrição, `NALU_X` para segredos.
- `secret` como tipo que nunca vira texto.
- `--dry-run` e `--verbose` automáticos; dry-run que roda de verdade dentro de pasta temporária.
- `os` como constante de compilação com eliminação do ramo morto.
- `duration` com unidades conferidas; `defer`; `parallel`; `retry`; `embed`; `native`; `test`/`assert`.
- Zero dependências em Rust; diagnósticos estilo rustc com "você quis dizer"; `check --json`.
- Checagem de dependências no início do script; quoting centralizado; sem `set -e`.

## O que a Nalu muda

| Tema | Stela | Nalu | Por quê |
|---|---|---|---|
| **Erros** | `throws` na assinatura; **toda** chamada falível com `try` prefixo ou `??`; `??` com 3 sentidos (`valor`, `throw`, `{}`) | **Falha rápida por padrão**; sem `throws`; recuperação só com **`or`** (`or valor`, `or fail`, `or ignore`, `or { }`); `try { } or { }` | O caso comum (propagar) custa zero tokens; o compilador conhece cada ponto falível e cuida da propagação. Um script de deploy de 90 linhas perde ~25 `try`/`throws` |
| Interpolação | `{{x}}` | `{x}` (`{{` literal) | Python, Rust, C#, Swift; menos ruído; mesmo em comandos |
| Declaração | `const` / `let` | `let` / `var` | Mais curto; `let` imutável como Rust/Swift |
| Função | `func f(a: T): R throws` | `fn f(a: T) -> R` | |
| Seleção | `switch` / `case a, b =>` / `default` | `match` / `a \| b =>` / `_` | Rust; `match` é expressão |
| Condicional-valor | ternário `c ? a : b` | `if c { a } else { b }` como expressão | uma forma só |
| Import | `import fs, log` (sem uso = erro) | **sem import** | zero cerimônia |
| Laço | `for x of xs` | `for x in xs`, `0..=10` | |
| Nomes | camelCase (`toUpperCase`) | snake_case (`upper`), semântica Python/Rust (`replace` = todas) | consistência com Rust/Python |
| Struct literal | `{ … }` só com tipo no contexto; `({ … })` em `case` | `Tipo { … }` sempre válido; `{ … }` quando o tipo é conhecido | sem truque de parênteses |
| Mapas | só `m.get(k) ?? x`; `m[k]` é erro | `m[k] or x`, `m[k] = v` | leitura natural; continua falível |
| Variável não usada | **erro** | **aviso** `W001` | edição iterativa sem cascata |
| Comando como condição | não existe | `if $ git diff --quiet {` | o idioma mais comum do shell |
| Positional args | erro "opção desconhecida" | `args: str[]` | `./x.sh arq1 arq2` |
| Funções e globais | só leitura | leem; escrevem em `var` do topo | contadores sem gambiarra |
| Parser | para no 1º erro de sintaxe | resiliente: vários erros | editores e agentes |
| Códigos de erro | não | `E012`, `W001` | `--json`, docs, busca |
| Linux × macOS | o mesmo Bash 3.2 para os dois | `linux` usa Bash 4.3+ (`declare -A`, `wait -n`, `${x,,}`); `macos` fica no 3.2 | mapas O(1) nativos e menos runtime no Linux |
| `parallel` no Windows | sequencial | jobs (meta v0.1; mínimo: sequencial) | |
| PowerShell 5.1 | defeitos documentados (aspas em argumentos, encoding, `echo`, stderr) | resolvidos no gerador | é a vantagem de ser compilador |
| Geradores | dois (3.100 + 1.600 linhas) que repetem o passeio da AST | **IR** + dois backends finos | equivalência estrutural |
| Runtime | strings Rust escapadas | arquivos `.sh`/`.ps1` reais (`include_str!`) | lint, teste, diff legível |
| Texto | `[[ a < b ]]` (locale) × `-clt` (cultura) | ordinal nos dois | igualdade de resultado |

## O mesmo script nas duas

Stela (`examples/backup/backup.stela`, 60 linhas) — trecho:

```
func compactar(pasta: string, saida: path): int throws {
    if !fs.exists(pasta) {
        throw "a pasta {{pasta}} não existe"
    }
    try $ tar -czf {{saida}} {{pasta}}
    return saida.size
}

func compactarOuZero(pasta: string, saida: path): int {
    try {
        return compactar(pasta, saida)
    } catch (err) {
        log.error("falhou ao compactar {{pasta}}: {{err}}")
        return 0
    }
}

const tmp = fs.tempDir() ?? throw "não consegui criar a pasta temporária"
defer fs.remove(tmp) ?? {}
const tamanhos = parallel(max: 4) for pasta of pastas {
    compactarOuZero(pasta, tmp / "{{pasta}}.tgz")
}
```

Nalu (`examples/backup.nalu`, 40 linhas) — o mesmo trecho:

```
fn compactar(pasta: str, saida: path) -> int {
    if !fs.exists(pasta) {
        fail "a pasta {pasta} não existe"
    }
    $ tar -czf {saida} {pasta}
    return saida.size
}

let tmp = fs.temp_dir() or fail "não consegui criar a pasta temporária"
defer fs.rm(tmp)
let tamanhos = parallel(max: 4) for pasta in pastas {
    compactar(pasta, tmp / "{pasta}.tgz") or {
        log.error("falhou ao compactar {pasta}: {err}")
        0
    }
}
```

## Como medimos "ganhar"

| Eixo | Métrica | Como |
|---|---|---|
| **Sintaxe** | tokens e linhas nos 4 exemplos portados (`ola`, `backup`, `deploy`, `release`); palavras-chave; conceitos | contagem direta; meta: −25% tokens |
| **Performance** | tempo de execução dos scripts gerados em laços, mapas e listas (`tests/bench/`) no Bash 3.2 e no Bash 5; tempo de compilação | `time`; meta: igual ou melhor no macOS, 2× melhor no Linux em mapas |
| **Resultado** | casos-limite de QA da Stela (`exercicios/qa/casos`) portados: quantos dão a **mesma** saída Bash × PowerShell; defeitos do 5.1 resolvidos; `parallel` real no Windows | `tests/cases` com oráculo diferencial |
| **DX** | erros por passada, códigos, `--json`, avisos × erros | casos em `tests/errors` |

A honestidade da Stela ("o que não dá, é erro de compilação") é a régua: a Nalu não ganha fingindo que algo funciona.

## Números (27/10/2026)

Medidos nesta máquina (macOS, `/bin/bash` 3.2). Programas em `tests/bench/` (`tests/bench/bench.sh`); contagem de tokens em `examples/` versus `../mis/examples/` (sem comentários e linhas vazias).

### Sintaxe: os quatro exemplos portados

| Exemplo | Linhas Nalu | Linhas Stela | Tokens Nalu | Tokens Stela |
|---|---|---|---|---|
| ola | 15 | 16 | 86 | 94 |
| backup | 32 | 43 | 193 | 254 |
| deploy | 58 | 61 | 354 | 374 |
| release | 49 | 59 | 388 | 431 |
| **total** | **154** | **179** | **1021** | **1153** |

Redução: 14% em linhas, 12% em tokens, 13% em caracteres. O ganho vem da ausência de `try`/`throws`/`import` e do `or` com bloco (sem função auxiliar `xOuZero`). A meta de −25% não foi atingida nos exemplos pequenos; o ganho cresce com o número de chamadas falíveis (backup: −24% tokens).

### Performance dos scripts gerados (Bash 3.2, tempo de parede)

| Caso | Stela | Nalu | |
|---|---|---|---|
| `laco` (20k iterações, `+=`, interpolação) | 0,123 s | 0,120 s | empate: os dois geram o mesmo laço mínimo |
| `mapa` (2k chaves, 4k set/get, mapa devolvido por função) | 1,133 s | **0,280 s** | **4× mais rápido**: uma variável por chave, O(1) em qualquer tamanho |
| `lista` (5k structs, filter/map/sort/first/last) | 0,470 s | 0,445 s | empate |
| compilar `deploy` para os 3 alvos | 5 ms | 6 ms | empate |

### Resultado (correção dos alvos)

| Item | Stela | Nalu |
|---|---|---|
| Casos de teste executados nos dois shells com saída idêntica | 23 casos (`tests/cases`) | 41 casos + pacote (`tests/cases`), 75 casos de erro |
| Aspas tipográficas em literais (quebra o `.ps1` da Stela, A5) | aberto | resolvido (`aspas`) |
| `echo` com um argumento por linha no PowerShell (M11) | aberto | resolvido (escrita nativa de uma linha) |
| Argumento vazio `""` para programa externo no 5.1 | limitação documentada | funciona (linha de comando montada por `Process`) |
| `2>` virando `NativeCommandError`; `>` com BOM/UTF-16 (M12) | aberto | resolvido (`[IO.File]`, UTF-8 sem BOM) |
| Comparação de texto por cultura × bytes (M2, M3) | divergente | ordinal nos dois |
| `string[][]` de 1 item achatado (bug 1 do exercício deploy) | aberto | regressão coberta (`aninhadas`) |
| `exit()` em função pulando `defer` do topo (A4) | aberto | resolvido (pilha global com marca) |
| Overflow via `double` na divisão (M8) | aberto | `[Math]::DivRem` |
| Parser para no 1º erro de sintaxe | sim | resiliente (6 erros em 6 linhas numa passada) |
| Variável não usada | erro | aviso |
| `parallel` no Windows | sequencial | sequencial (igual) |
| Alvo Linux com Bash 4+ | mesmo código do 3.2 | código próprio (`declare -A`, `wait -n`, listas por referência); executado nos testes com Bash 5 |

### O que a Stela ainda tem e a Nalu não

`regex`, `encoding` e `pkg` implementados (na Nalu são v0.2: o `check` recusa com `E090`); `stela fmt`; `stela vet` como comando separado (na Nalu os avisos de portabilidade saem no `check`); handbook HTML.

## Atualização (09/11/2026)

Depois da comparação acima, a Nalu (que se chamava Onda; ver D20) fechou parte da distância e ganhou coisas que a Stela não tem:

| Item | Stela | Nalu |
|---|---|---|
| `json` | planejado | `get`, `get_list`, `keys`, `has`, `quote`, o mesmo algoritmo nos três alvos (D24) |
| `hash`, `archive` | implementados | implementados (D24) |
| `http` | `get`, `status`, `download` | + `post`, `headers`, `bearer: secret` sem expor o token em argumentos (D24) |
| `fs.chmod` | não | sim; no Windows confere e não faz nada |
| Índice de lista fora da faixa | — | falha (D13) |
| `slice(0, -1)` | `"abcd"` (falso amigo do JS) | `"abc"`, como no JS (D21) |
| `.size` de caminho inexistente | 0 | falha, como `fs.size` (D22) |
| Quais linhas podem falhar | o `try` em cada linha mostra | `check --explain` por linha e extensão do VS Code (D23) |
| Passar lista de 3.000 itens a uma função (Bash 5) | ~6 ms por chamada | ~0 (por referência no alvo linux, D25) |
| `.lower()`/`.upper()` no Bash 3.2 | um `tr` por chamada (~2,5 ms) e perde a quebra de linha final | sem processo para ASCII e Latin-1 (14× mais rápido) |
| Alvos executados nos testes | Bash 3.2 (+ pwsh quando há) | Bash 3.2, Bash 5 e PowerShell 7 |
