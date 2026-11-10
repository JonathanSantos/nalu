# SPEC-003 — Backend PowerShell 5.1 e runtime

| Campo | |
|---|---|
| Dono | agente C |
| Status | 🟢 entregue (aguarda aceite do orquestrador) |
| Depende de | `src/ir.rs`, `src/stdlib.rs`, `src/target.rs` (congelados); [02-linguagem.md](../02-linguagem.md); [04-arquitetura.md](../04-arquitetura.md) |
| Arquivos | `src/ps/mod.rs` (e submódulos em `src/ps/`), `runtime/ps/*.ps1` |

## Objetivo

`nalu::ps::generate(&program, mode)` devolve um script para **Windows PowerShell 5.1** (que também roda no PowerShell 7) com a **mesma saída observável** do Bash para o mesmo programa, resolvendo no gerador os defeitos conhecidos do 5.1; `launcher(name)` devolve o `.cmd`.

## Contrato

```rust
// src/ps/mod.rs
pub fn generate(program: &crate::ir::Program, mode: crate::Mode) -> String;  // texto com \n; o CLI converte para CRLF e põe o BOM
pub fn launcher(script_name: &str) -> String;  // "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0NAME.windows.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n"
```

- Só sintaxe do **5.1**: sem ternário, `??`, `??=`, `&&`/`||` entre comandos, `?.`, `?[]`, `ForEach-Object -Parallel`, `Join-String`.
- Mesma estrutura do script Bash (cabeçalho, prelúdio, runtime, parâmetros, dependências, funções, corpo). `Mode::Tests` com a mesma saída textual (`ok      nome`).

## Escopo v0.1

- [x] Prelúdio: `Set-StrictMode -Version 2`, `$ErrorActionPreference = 'Stop'`, `[Console]::OutputEncoding = [Text.Encoding]::UTF8`, `$OutputEncoding` UTF-8; script inteiro em `try { } catch { __Fail }` com `finally` para `defer`
- [x] Runtime sob demanda: `runtime/ps/*.ps1` com marcadores `# @fn Nome` / `# @needs A B`; `include_str!`; prefixo `__Nalu` ou `__`; ordem determinística
- [x] Parâmetros: leitor manual de `$args` (não `param()` do PS: precisamos de `--x`, `--x=v`, listas `a,b`, `--`), `--help` imprime `program.usage` textual, erros código 2; `secret` de `$env:NALU_X`; `args` posicionais
- [x] Dependências com `Get-Command`, código 127
- [x] Tipos: `[long]` para `int`/`duration`; `bool` → `$true/$false` mas **interpolado como `true`/`false`**; listas como `[System.Collections.Generic.List[object]]` ou arrays com `,@()` (escolha uma e seja consistente; cuidado com desenrolar de 1 item); structs como `[pscustomobject]` ou hashtable (cópia profunda em `Let`/`Assign`); mapas `[ordered]@{}` copiados; enums como texto
- [x] `Expr`: aritmética `[long]` com `[Math]::DivRem`/truncamento sem passar por `double`; comparação de texto **ordinal** (`[string]::CompareOrdinal`, `[string]::Equals`); `Len` por **code point** (contar `[char]::IsHighSurrogate`); `Interp` com `-f` ou concatenação (escapar `` ` `` `"` `$`); `PathJoin` com `Join-Path`
- [x] `Rvalue::Call` para **toda** entrada `v1` de `STDLIB`, `METHODS`, `PATH_PROPS`, `BUILTINS` (teste de cobertura lendo as tabelas); `Lambda` como scriptblock
- [x] `OnFail` nas cinco formas; `CurrentErr` = objeto `__Err` com `message`/`code`; funções falíveis lançam (`throw`) um objeto de erro próprio (não `string`) para carregar `code`
- [x] `Command`: `& prog args` com **escape manual de `"` para programas nativos** (`\"`, e `\\` antes de `\"`), argumento vazio documentado; `$ErrorActionPreference = 'Continue'` ao redor do nativo e `$LASTEXITCODE` conferido; `echo` → escrita nativa de uma linha com os args unidos por espaço; pipes entre nativos com `cmd /c`? **Não**: use pipeline do PS e confira `$LASTEXITCODE` do último (documente); redirecionamentos `>`/`>>` escrevendo **UTF-8 sem BOM** e `\n` via `[IO.File]`; `2>` sem virar `NativeCommandError`; `stdin` via `System.Diagnostics.Process` com `StandardInput` sem `WriteLine` extra (ou documente o `\n` extra se inviável para pipelines)
- [x] `--verbose`/`--dry-run` idênticos ao Bash (`+ cmd`, `[dry-run] cmd`, `******`)
- [x] `If`, `While`, `ForRange` (`for ($i = a; $i -lt b; $i++)`, nunca `a..b`), `ForList` (cópia), `ForMap`, `Match` (`switch` com `break` em cada braço; faixas como `if`), `Break/Continue/Return/Exit`
- [x] `Defer` com `try/finally` por função e pilha no topo; `exit()` dentro de função roda todos
- [x] `Retry`: laço + `Start-Sleep -Milliseconds`
- [x] (sequencial) `Parallel`: **meta**: `Start-Job` com limite `max`, resultados na ordem, erros agregados; **mínimo aceitável** v0.1: sequencial com a mesma semântica (marque no relatório qual ficou)
- [x] `Native { powershell }` literal
- [x] `fs.*`: `[IO.File]::ReadAllText` com detecção **desligada** (ler bytes como UTF-8 e tirar BOM), `WriteAllText(p, s, UTF8Encoding($false))`, `Remove-Item -Recurse -Force` com as mesmas guardas do Bash, `fs.glob` sem dotfiles/sem listar conteúdo de pasta, `fs.walk` relativo e ordenado com `/`
- [x] Literais: `sq()` dobra `'` **e** `‘ ’ ‚ ‛`; `dq` escapa `“ ” „` também (aspas tipográficas fecham strings no PS!)
- [x] `duration_str`, `to_int` (`[long]::TryParse` com `NumberStyles` que recuse `+5`? Bash recusa; alinhe), `str` methods por code point, `sort` ordinal, `time.format` só com o subconjunto (recuse `%` desconhecido em runtime)
- [x] `Mode::Tests`

## Fora de escopo

Bash, CLI, parser. `regex`, `hash`, `encoding`, `archive`, `pkg` (v0.2; gere `__Unimplemented 'x'` com falha em runtime).

## Critérios de aceite

1. `cargo test` passa, incluindo testes que geram scripts a partir de IR feita à mão e **validam a sintaxe** (sem `pwsh` nesta máquina: valide com um verificador próprio mínimo — balanceamento de `{ } ( )`, aspas, ausência das construções do PS 7 listadas — e deixe um teste que roda o script quando `NALU_PWSH` ou `pwsh`/`powershell` existir).
2. Nenhuma construção só do PowerShell 7 em `runtime/ps/*.ps1` nem no gerado (teste por regex: `\?\?`, `\?\.`, `\|\|`, `&&` fora de strings, `-Parallel`).
3. Cobertura da stdlib (teste lendo as tabelas).
4. `launcher("x")` igual ao contrato, com CRLF.
5. Casos-limite do QA da Stela (`../mis/exercicios/qa/casos/`) revisitados: aspas tipográficas, `echo`, `2>`, BOM, ordinal, `string[][]` com 1 item, `--vezes 3` separado. Para cada um, diga no relatório como o gerador resolve.

## Testes

- `src/ps/tests.rs`: IR à mão → `.ps1`; verificador de sintaxe mínimo; execução condicional com `NALU_PWSH`.
- `runtime/ps/tests.ps1` (roda quando houver pwsh).

## Notas de implementação

- A referência de armadilhas está em `../mis/exercicios/qa/RELATORIO.md` (seção 2) e em `../mis/docs/insights/2026-10-27-analise-clinica.md` §8.3. Leia antes de começar.
- Escape de argumentos para nativos no 5.1 (bug PowerShell#1995): para cada argumento, se contém `"` ou espaço ou está vazio, envolva em `"…"` com `\"` para aspas internas e dobre `\` que precedem `"`. Encapsule em `__Arg`.
- Retornar listas: `return ,$list` **e** nunca envolver chamadas em `$( )` ao atribuir (`$x = fn_um` direto; para capturar saída de comando use `[string[]]`/`Out-String` com cuidado).
- Funções do usuário: `function fn_nome([tipo]$a, …)`; descarte saída que não é retorno (`| Out-Null` em chamadas nativas dentro de funções, ou atribua a `$null`).
- Erros: `class` não existe no 5.1 de forma confiável para `throw`; use `[pscustomobject]@{ message=…; code=… }` em `throw`, e no `catch` leia `$_.TargetObject` ou `$_.Exception.Message` conforme o tipo (padronize em `__ErrOf($_)` que converte qualquer exceção, inclusive do .NET, para `{message, code}`; comandos nativos usam `$LASTEXITCODE`).
- `exit` com código > 255: aplique `% 256` para bater com o Bash.
- `.ps1` final: o CLI (SPEC-004) grava BOM + CRLF; `generate` devolve `\n`.
- `Get-ChildItem` lista o conteúdo quando o caminho é pasta: para `fs.glob` use `Resolve-Path`/`[IO.Directory]::GetFileSystemEntries` com o padrão no último componente.
- Mantenha métodos pequenos.

## Relatório

**Estado.** `src/ps/` (`mod.rs`, `gen.rs` ~1.700 linhas, `runtime.rs`, `syntax.rs`, `tests.rs`) e `runtime/ps/*.ps1`
(8 arquivos, ~75 helpers com `# @fn`/`# @needs`, incluídos sob demanda por fechamento transitivo, em ordem
fixa). Toda entrada `v1` de `STDLIB`, `METHODS`, `PATH_PROPS` e `BUILTINS` tem tradução (teste
`cobertura_da_stdlib`); `regex`, `hash`, `encoding`, `archive`, `pkg` geram `__Unimplemented 'x'` (falha em
runtime com mensagem clara).

**`parallel`: sequencial** (o mínimo da v0.1), com a mesma semântica: todas as tarefas rodam, resultados na
ordem da lista, erros juntados em `parallel: e1; e2` → `on_fail`; `max` é avaliado e ignorado. `Start-Job` ficou
de fora de propósito: cada job é outro processo (≈1 s de partida no Windows), não enxerga as funções do script
nem as variáveis do topo, e a saída de `print`/programas externos dentro dele não chega ao console na ordem.
Um caminho real seria um `RunspacePool` (threads no mesmo processo) copiando as definições — fica para a v0.2.

**Modelo de execução** (decisões que o revisor precisa conhecer):
- Variáveis `$o_nome` (maiúsculas codificadas: o PowerShell não distingue `a` de `A`); globais em funções
  via `$script:`; lambdas e `defer` viram funções `__lam_N`/`__defer_N` (não scriptblocks com
  `GetNewClosure`, que perdem o acesso às funções do escopo do script).
- Funções do usuário devolvem em `$script:__ret` e são chamadas **como instrução**: nada de `$x = fn` nem
  `$( )`. A saída de comandos dentro delas vai para o console, e listas de 1 item nunca se desenrolam.
- Falhas são exceções com `Data['nalu_code']` (`__Throw`); `__ErrOf` converte qualquer erro (Nalu, .NET,
  `MethodInvocationException`, `AggregateException`) em `{message, code}`. `Propagate` não gera código: a
  exceção sobe até o `catch` certo (`try`, `retry`, `parallel`, função, topo). Os outros `OnFail` viram
  `try { } catch { }` só em volta da operação, então `break`/`continue`/`return` do handler funcionam.
- `match` vira `if/elseif` (um `break` dentro de `switch` pararia o `switch`, não o laço); faixas são `-ge/-lt`.
- `defer`: pilha `$__defers` por função (só nas que têm `defer`) + `try/finally`; os valores lidos pela
  instrução são capturados na hora. `exit` roda os `finally` (conferido no pwsh), então `exit()` dentro de
  função roda os `defer` da função e do topo.
- `sys.cd` muda `Set-Location` **e** `[Environment]::CurrentDirectory` (o .NET e os processos filhos usam a do
  processo); numa função que usa `sys.cd`, a pasta volta no `finally`.

**Armadilhas do 5.1 resolvidas, e como** (casos do QA da Stela entre parênteses):
1. Aspas tipográficas fecham strings (A5): `lit()` dobra `' ‘ ’ ‚ ‛` em aspas simples; texto com controle vai
   em aspas duplas com `` ` `` antes de `" “ ” „`, `$` e crase. Caso `aspas`, teste `literais_escapam_aspas_tipograficas`.
2. Argumentos de programas nativos (#1995, aspas internas somem, `""` some): **nenhum** programa externo passa
   por `& prog`. Todos rodam por `System.Diagnostics.Process` com a linha de comando montada por `__QArg`
   (regras do MSVCRT: `\"`, `\` dobradas antes de `"` e no fim, `""` para vazio). O argumento vazio **funciona**
   (a limitação da §26 deixa de existir no gerado). `.cmd`/`.bat` vão por `cmd.exe /d /s /c` (lá o `%VAR%` é
   expandido pelo cmd: documentado em `cmd.ps1`).
3. `echo` um argumento por linha (M11): `$ echo …` vira uma escrita de uma linha (args unidos por espaço; `-n`
   respeitado), também como primeiro estágio de pipe e com `>`. Caso `echo`.
4. `2>` virando `NativeCommandError` / stderr com `ErrorActionPreference=Stop`: stderr é herdado (bytes direto
   no console) ou copiado byte a byte para o arquivo/stdout (`2>`, `2>>`, `2>&1`); o PowerShell nunca vê o
   stderr do filho.
5. Encoding (M4, M12): `.ps1` com BOM e CRLF (o CLI); `[Console]::OutputEncoding`/`$OutputEncoding` em UTF-8
   **sem** preâmbulo (o `[Text.Encoding]::UTF8` do .NET Framework escreveria BOM no stdout redirecionado);
   `fs.read` lê bytes e decodifica UTF-8 sem detecção (nada de UTF-16 pelo BOM), tirando o BOM; `fs.write`,
   `>`/`>>` gravam UTF-8 sem BOM e o texto/bytes exatos; `fs.replace` preserva o BOM se havia; `print`/`log`
   escrevem `\n` (não `\r\n`).
6. Comparação por cultura (M2, M3): `==` é `[string]::Equals` (ordinal), `<`… é `__Cmp` (ordinal por code
   point, corrigindo surrogates × U+E000..FFFF, B11); `starts_with`/`ends_with`/`index_of` com
   `StringComparison.Ordinal`; `sort` com `StringComparer.Ordinal`; mapas e structs em `OrderedDictionary`
   com `StringComparer.Ordinal` (o `@{}` ignora maiúsculas: `{"api", "API"}` seriam uma chave só).
7. `string[][]` de 1 item achatado por `$( )` (aninhadas): retorno por `$script:__ret`, helpers devolvem
   `return ,$l`, nunca `$( )` em volta de valor. Caso `aninhadas`, teste `listas_lambdas_e_um_item`.
8. `--vezes 3` separado (A6): leitor manual de `$args` (`__ParseArgs`) com `--x v`, `--x=v`, bool, listas, `--`;
   valores validados (`int` recusa `+5` e fora de 64 bits; duração na gramática dos literais; enum).
9. Overflow via `double` na divisão (M8): `/` é `[long][decimal]::Truncate([decimal]a / [decimal]b)` (exato em
   64 bits). Soma/multiplicação que estouram ainda viram `double` (a §26 diz "indefinido").
10. stdin com `\n` extra (M10): `valor | $ cmd` escreve os bytes UTF-8 exatos no `StandardInput` e fecha.
11. Captura `$( )` (B4): stdout em bytes → UTF-8 → tira **todas** as quebras finais (`\n` e `\r\n`).
12. Inteiro como argumento: em modo de comando `fn 5L` passa o **texto** `"5L"`; todo literal sai entre
    parênteses (`(5L)`) — achado executando no pwsh.
13. `$HOME`/`$home` é variável automática somente leitura (não use como nome); `exit` > 255 com `% 256` (B10);
    `fs.rm` com as guardas do Bash (`""`, `/`, `.`, `..`, `~`, a pasta pessoal, a pasta atual e as que a
    contêm; B9); `fs.glob` sem dotfiles e sem listar o conteúdo de pasta (M5); `fs.walk` relativo a `dir` como
    passado, com `/`, ordenado (M6); `path.ext` de dotfile é `""` (B1); `parent` da raiz é a raiz (B3);
    `time.format` só com o subconjunto, `%` desconhecido falha (M9); `Invoke-WebRequest` com TLS 1.2 e sem
    barra de progresso (`$ProgressPreference`), que deixa o 5.1 muito lento; nada de APIs do .NET 4.6
    (`DateTimeOffset.ToUnixTimeSeconds`): o WMF 5.1 roda com .NET 4.5.2.

**O que foi executado de verdade (PowerShell 7.6.9 no macOS, `NALU_PWSH`).**
- `cargo test --lib ps::`: 24 testes com IR feita à mão; cada script passa no verificador mínimo **e** em
  `tests/ps_syntax.ps1` (parser oficial recusando sintaxe do 7) e **roda**, comparando stdout e código:
  aritmética, strings por code point, listas/lambdas/1 item, mapas e structs, `OnFail` nas cinco formas,
  `try`, `defer` + `exit(300)` em função, `retry`, `parallel`, `match` com `break`/`continue`, parâmetros
  (`--help`, obrigatório, inválidos), comandos (aspas internas, argumento vazio, pipe, `>`, `2>`, stdin,
  captura, 127, pipefail), `--dry-run`/`--verbose` com segredo, `Mode::Tests`, `fs.*`.
- `runtime/ps/tests.ps1`: 60 verificações dos helpers.
- Corpus `tests/cases` pelo CLI: **40 de 41** casos passam no pwsh (os `.unix` só são validados); o que
  falha (`params_help`) é o `usage` do frontend sem `[args…]` (ver SPEC-004). `nalu test examples
  --target windows` 6/6; `tests/pacote` no Windows igual a `saida.txt`.
- Exemplos rodados no pwsh: `ola` (com `--nome Ana --vezes 2`), `deploy --dry-run`, `backup` (tar real,
  `parallel` com resultados, `defer`, `fs.move`) e `release` num repositório git de verdade (commit, tag,
  CHANGELOG em UTF-8 sem BOM com `\n`) — saída igual à do `.sh`.
- Benchmark (laço de 20.000 com `+=` e interpolação + mapa de 2.000 chaves e 4.000 `get`): 0,5 s no pwsh
  (o `.sh` no Bash 3.2: 0,46 s).

**O que só o Windows PowerShell 5.1 confirma** (não há 5.1 aqui): a montagem da linha de comando pelo
`CreateProcess` do .NET Framework (no pwsh/Unix o .NET usa as mesmas regras do MSVCRT para separar
`Arguments`, que foi o que rodou); programas `.cmd`/`.bat` via `cmd.exe`; `[Console]::OutputEncoding` num
console de verdade e com stdout redirecionado (está num `try`); o lançador `.cmd` repassando `%*` (inclusive
`--`, que o pwsh 7.6 preserva para o script); `Invoke-WebRequest`/TLS 1.2; `ui.*` com terminal; `sys.is_admin`;
`dirs.*` (`%APPDATA%`, `%LOCALAPPDATA%`); caminhos com `\` e letra de unidade (`__PathParent`, `fs.rm` de
`C:\`). O verificador mínimo (`check_syntax`) e o `ps_syntax.ps1` garantem só a **sintaxe** do 5.1; APIs
foram escolhidas à mão entre as do .NET 4.5.

**Pedidos fora do meu escopo.** Nenhuma mudança de contrato foi necessária. Para o frontend: `usage` sem
`[args…]` (a §18 mostra). Para o orquestrador: o modelo de execução acima merece entrar em
`docs/04-arquitetura.md` (hoje diz `& prog @args` e `switch`).
