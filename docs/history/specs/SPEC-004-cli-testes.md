# SPEC-004 — CLI, harness de testes e corpus de casos

| Campo | |
|---|---|
| Dono | agente C (junto com SPEC-003) |
| Status | 🟢 entregue (aguarda aceite do orquestrador) |
| Depende de | `src/lib.rs::compile`, `bash::generate`, `ps::generate`, `ps::launcher`, `diag`, `target` |
| Arquivos | `src/main.rs`, `tests/e2e.rs`, `tests/cases/*`, `tests/ps_syntax.ps1` |

## Objetivo

O binário `nalu` com `run`, `build`, `check`, `emit`, `test` e `--version`; um harness que compila e **executa** todo `tests/cases/*.nalu` nos shells disponíveis comparando saída e código; e um **corpus** de casos escrito a partir da referência, que serve de teste de aceitação para as outras três specs.

## Contrato

CLI exatamente como [02-linguagem.md §24](../02-linguagem.md#24-ferramentas-cli):

```
nalu run x.nalu [-- args]                 compila para Target::current() e executa (bash; pwsh/powershell no Windows)
nalu build x.nalu [--target a,b] [-o dir] grava dir/x.linux.sh, x.macos.sh (chmod +x), x.windows.ps1 (BOM+CRLF), x.windows.cmd
nalu check x.nalu [--json] [--explain]    diagnósticos; código 1 se erro
nalu emit x.nalu --target T [--ir]        script ou IR ({:#?}) no stdout
nalu test [pasta|arquivo] [--target T]    junta os .nalu da pasta (ou da pasta do arquivo), Mode::Tests, executa
nalu --version | -V ; nalu --help | -h
```

Códigos: 0 ok, 1 erros de compilação/testes/execução falhou na compilação, 2 uso errado da CLI (mensagem + `use nalu --help`). O código do script executado por `run` é repassado.

Parser de argumentos à mão (sem crates), mas **estrito**: flags em qualquer ordem, valor por `--x v` ou `--x=v`, `--` separa args do script, flag desconhecida = código 2.

## Escopo v0.1

- [x] `main.rs`: subcomandos acima; leitura do arquivo (erro legível se não existir); `embed_root` = pasta do arquivo; avisos sempre impressos (stderr), erros abortam; `--json` imprime só o JSON (stdout); `build` cria `-o` (padrão `dist`); `.ps1` com BOM `EF BB BF` e `\r\n`; `.sh` com `0o755` no Unix
- [x] `run`: gera para o alvo atual num arquivo temporário (`std::env::temp_dir()/nalu-<pid>/x.sh`), executa com `bash` (`/bin/bash` se existir) ou `pwsh`/`powershell`, repassa stdin/stdout/stderr e o código; apaga o temporário
- [x] `test`: junta os arquivos da pasta (ordem alfabética; concatena os textos com um comentário `// --- arquivo` entre eles e um mapa de offsets para os diagnósticos apontarem o arquivo certo, **ou** compile cada um e junte as IRs: escolha, documente); roda; repassa o código
- [x] `tests/e2e.rs`: para cada `tests/cases/x.nalu`: `build --target macos` (nesta máquina) ou `linux` (em Linux), executa com `bash`, compara stdout com `x.out` (e código com `x.code`, padrão 0; argumentos de `x.args`, um por linha); gera também o `.ps1` e, se `NALU_PWSH` ou `pwsh`/`powershell` existir, valida com `tests/ps_syntax.ps1` (parser oficial recusando sintaxe do PS 7) e executa comparando com `x.windows.out` ou `x.out`; `bash -n` em todo `.sh`
- [x] `tests/e2e.rs`: para cada `tests/errors/x.nalu`: `check` falha e o stderr contém o código da primeira linha (`// error: E0xx`); se a primeira linha for `// warning: W0xx`, `check` passa e o stderr contém o código
- [x] `tests/e2e.rs`: `examples/*.nalu` compilam para os três alvos; `bash -n`; `nalu test examples` passa
- [x] **Corpus** `tests/cases/` (≥ 25 casos, cada um focado, com `.out` escrito **a partir da referência**, antes de existir implementação): `nucleo` (let/var/if/for/while/match), `strings` (métodos, interpolação, `{{`, `'cru'`, `"""`), `numeros` (truncamento, `%`, `1_000`, `0xFF`, `to_int` falhando), `duracao` (literais, aritmética, texto, `/ 1s`), `listas` (métodos, lambdas, sort ordinal, slice), `mapas` (literal, `[k]`, `or`, `has`, `remove`, ordem, `for k, v`), `structs` (literal com/sem nome, `..base`, métodos, `self`, listas de structs, campo lista), `enums` (`match` exaustivo, `parse`, texto), `erros` (`fail`, `or` nas 5 formas, `try/or`, propagação em função, `err.code`, código de saída do script), `comandos` (`$`, `$( )`, pipe, redirecionamentos, `{lista}` spread, `if $ cmd`, `or ignore`, `valor | $`), `params` (`.args` com `--x v`, `--x=v`, bool, lista, enum, duração, obrigatório faltando → `.code` 2, `--help` → `.out`), `dryrun` (`--dry-run` com `.args`; escritas mostradas; temp_dir real), `verbose`, `defer` (ordem inversa, em falha, `exit` dentro de função), `retry`, `parallel` (ordem dos resultados, `max`, erro agregado), `funcoes` (padrões, nomeados, recursão, listas/structs por valor, globais `var`), `escopo` (sombreamento, globais em funções), `fs` (em `temp_dir`: write/read/append/ls/walk/glob/rm/mkdir/copy/move/replace/size/props), `path` (props, `/`, `~`), `os` (`match os` com saída diferente por alvo → `.windows.out`), `unicode` (`len` por code point, `upper` de `ação`, ordinal `"a" < "B"`), `aninhadas` (`str[][]` com 1 item: regressão do bug da Stela), `secret` (mascarado no verbose; `.args` com `--dry-run --verbose`), `testes` (pasta `tests/pacote/` com um teste que falha de propósito e `saida.txt`)
- [x] `tests/ps_syntax.ps1`: usa `[System.Management.Automation.Language.Parser]::ParseFile` e recusa tokens só do PS 7

## Fora de escopo

Gerar código (SPEC-002/003), frontend (SPEC-001). Não escreva implementação de linguagem no `main.rs`.

## Critérios de aceite

1. `cargo build --release` gera `target/release/nalu`; `nalu --help` lista os subcomandos; `nalu check inexistente.nalu` dá código 1 com mensagem legível; `nalu foo` dá código 2.
2. `cargo test` roda o harness; enquanto as outras specs não estão prontas, os casos **falham com mensagem clara** (não com pânico): o harness deve agregar e listar todos os casos que falharam, com diff de saída.
3. Os `.out` do corpus foram escritos à mão a partir da referência (não copiados da saída do compilador). Onde a referência for ambígua, anote a dúvida em `tests/cases/README.md` para o orquestrador decidir.
4. `nalu build examples/ola.nalu` gera os 4 arquivos; o `.ps1` começa com BOM e tem CRLF; o `.cmd` é o do contrato.

## Testes

O harness **é** o teste. Mais: testes de unidade do parser de argumentos da CLI em `main.rs`.

## Notas de implementação

- Para `run`, procure `bash` em `/bin/bash` → `bash` no PATH; no Windows, `pwsh` → `powershell`. Mensagem clara se nenhum existir (`não achei pwsh nem powershell; defina NALU_PWSH`).
- O harness: colete falhas em `Vec<String>` e faça um único `panic!` no fim com todas, para o relatório mostrar tudo de uma vez.
- Para casos com saída dependente de alvo, `x.windows.out`; para casos que não devem rodar no PowerShell (usam `head`, etc.), `x.unix` marcador vazio.
- `tests/cases/README.md` documenta a convenção.
- Mantenha `main.rs` pequeno: um `enum Cmd`, `fn parse_args`, `fn cmd_build` etc.

## Relatório

**CLI (`src/main.rs`).** `run`, `build`, `check` (`--json`, `--explain`), `emit` (`--target`, `--ir` com
`{:#?}`), `test` (`--target`), `--version`/`-V`, `--help`/`-h`. Parser à mão e estrito: flags em qualquer
ordem, `--x v` e `--x=v`, `--` só em `run`, flag ou subcomando desconhecido, valor faltando, alvo inválido ou
arquivo a mais → código 2 com `use nalu --help para ver os comandos` (6 testes de unidade). Decisões:
- `check` compila para os **três** alvos e junta os diagnósticos sem repetir (o `W010` depende do alvo);
  `--json` imprime só a lista no stdout; `--explain` lê a IR (`fn x: pode falhar por: fail, .to_int()`),
  porque o frontend não expôs um `explain` (a SPEC-001 deixava a escolha).
- `build`: avisos impressos uma vez (da primeira compilação); `.sh` com `0o755`; `.ps1` com BOM e CRLF (sem
  dobrar `\r\n`); `.cmd` do contrato; `-o` padrão `dist`. Fonte com BOM é aceita.
- `run`: `std::env::temp_dir()/nalu-<pid>/x.<alvo>`; `/bin/bash` → `bash` no PATH; no Windows `NALU_PWSH` →
  `pwsh` → `powershell`; stdin/stdout/stderr herdados; código repassado (sinal → 128+n); temporário apagado.
- **`test`: pacotes, não a pasta inteira.** Juntar todos os `.nalu` da pasta não funciona com `examples/`
  (quatro scripts independentes, cada um com os seus `param`). Um pacote é `x_test.nalu` + `x.nalu` (se
  existir) ou um arquivo com os próprios `test`; os textos são concatenados (quem tem `param` primeiro) com
  `// --- arquivo` entre eles e um mapa de deslocamentos devolve cada diagnóstico ao arquivo certo. Cada
  pacote roda em `Mode::Tests`; o código final é o pior. **Dúvida para o orquestrador**: a §23 diz "junta os
  `.nalu` da pasta".
- Saída no stdout tolera `EPIPE` (`nalu emit x | head` não entra em pânico).

**Harness (`tests/e2e.rs`, 10 testes).** Casos em paralelo (até 8 threads), tempo limite de 60 s por
execução, falhas juntadas num único `panic!` com diff `-`/`+` e o stderr. Convenção em
`tests/cases/README.md`: `x.out`, `x.<alvo>.out`, `x.code`, `x.args`, `x.err` (linhas exigidas; `!linha`
proibida), `x.env`, `x.unix`. Para cada caso: `build` linux+macos, `bash -n` nos dois, execução do alvo da
máquina; `.ps1` com BOM/CRLF e o verificador mínimo; com PowerShell, `tests/ps_syntax.ps1` e execução.
Também: `tests/errors` (75 arquivos do agente A, todos passam), `examples` nos três alvos (`_test.nalu` de
fora; `.cmd` do contrato; parser oficial no `.ps1`), `nalu test examples` (Bash e, com pwsh, Windows),
`tests/pacote`, e a CLI (ajuda, versão, código 2, arquivo inexistente → 1, `build` gera os 4 arquivos,
`run` repassa argumentos, `check --json`). `tests/ps_syntax.ps1`: parser oficial + recusa de ternário, `??`,
`??=`, `&&`/`||`, `?.`/`?[]`, `-Parallel`, `Join-String` + exige BOM.

**Corpus: 41 casos** em `tests/cases` (a lista mínima da spec inteira, mais `falha_topo`, `echo`, `deps`,
`params_falta`, `params_invalido`, `params_help`, `secret_stdin`, `log`, `env`, `env_filho`, `tempo`,
`sistema`, `exit`, `exit_zero`, `aspas`, `native`, `match`; `path` cobre o item "path"), cada `.out` escrito à
mão a partir da referência, e `tests/pacote/` (`calc.nalu`, `calc_test.nalu` com um teste que falha de
propósito e um erro não tratado, `saida.txt`). 18 dúvidas da referência em `tests/cases/README.md`.

**Estado no fim (macOS, Bash 3.2 + pwsh 7.6.9).** `cargo test --lib` e `--bin` verdes; e2e 7 de 10 verdes.
Os 3 vermelhos são divergências **das outras specs** em relação à referência, apontadas pelo corpus:
- frontend (SPEC-001): `usage` sem ` [args…]` na linha `uso:` (§18) → `params_help` falha nos dois alvos;
- Bash (SPEC-002): `fs.glob(".*")` devolve `.` e `..`; `xs[99]` de `int[]` imprime vazio em vez de `0`
  (§26); `"+5".to_int()` é aceito (a SPEC-003 diz que o Bash recusa); `("~/algo": path).parent ==
  dirs.home()` dá `false`; `nalu test` imprime uma linha vazia antes do resumo.
- PowerShell: 40/41 casos (só `params_help`), pacote e exemplos verdes.
