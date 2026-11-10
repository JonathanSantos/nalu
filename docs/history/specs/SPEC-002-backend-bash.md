# SPEC-002 — Backend Bash (Linux 4.3+ e macOS 3.2) e runtime

| Campo | |
|---|---|
| Dono | agente B |
| Status | 🟢 pronto para aceite (Linux só validado por `bash -n` + revisão: ver Relatório) |
| Depende de | `src/ir.rs`, `src/stdlib.rs`, `src/target.rs` (congelados); [02-linguagem.md](../02-linguagem.md) (semântica observável); [04-arquitetura.md](../04-arquitetura.md) (modelo de execução) |
| Arquivos | `src/bash/mod.rs` (e submódulos em `src/bash/`), `runtime/bash/*.sh` |

## Objetivo

`nalu::bash::generate(&program, mode)` devolve um script Bash **legível, correto e rápido** para `program.target` (`Linux` ou `Macos`), implementando toda a semântica da referência e toda a stdlib `v1`, usando só o que o Bash daquele alvo tem.

## Contrato

```rust
// src/bash/mod.rs
pub fn generate(program: &crate::ir::Program, mode: crate::Mode) -> String;
```

- `program.target == Target::Linux`: pode usar Bash **4.3+** (`declare -A`, `${x,,}`/`${x^^}`, `mapfile`, `wait -n`, `declare -n` evite, `read -d ''`, `printf -v`, `[[ =~ ]]`, `${x:offset:len}`). Ferramentas: GNU coreutils **ou** BusyBox (`stat -c`, `date -d`, `base64 -d`, `sort -z`, `find`, `mktemp -d`).
- `program.target == Target::Macos`: só Bash **3.2** (`/bin/bash` desta máquina: **teste nele**). Sem `declare -A`, `${x,,}`, `mapfile`, `wait -n`, `local -n`, `&>>`, `|&`, `coproc`. Ferramentas BSD (`stat -f %z`, `date -r`, `base64 -D`/`--decode`, `shasum -a 256`, `sed -i ''`, `mktemp -d -t`).
- `Mode::Script`: cabeçalho, prelúdio, runtime necessário, leitor de parâmetros, checagem de dependências, funções, corpo de topo, `trap` dos `defer`.
- `Mode::Tests`: prelúdio, runtime, funções, `let`/`var`/`param`(padrão) do topo que não têm efeito (o lowering já garante que `Program.body` em modo teste só tem isso? **Não**: o backend ignora `Stmt::Command`, `Call` com `writes`, e roda só `Let` cujo `Rvalue` é `Expr`/`List`/`Map`/`Struct`), depois um bloco por `TestDef`: imprime `ok      nome` ou `FALHOU  nome: motivo`, depois (sem linha em branco) o resumo `N teste(s), M falha(s)`, código 1 se `M > 0`.
- Entrada da IR: siga as regras do topo de `ir.rs`. Você pode **confiar** que `Expr` é pura, que nomes são únicos, que `Call.args` é posicional e completo.
- Saída: `String` terminada em `\n`, começando com `#!/usr/bin/env bash` e um comentário `# Gerado por nalu <versão> a partir de <arquivo> (alvo: linux|macos). Não edite.`. Cada `Stmt` de topo e de função precedida por `# linha N: ` **só** quando `N` muda (legibilidade).

## Escopo v0.1

- [x] Prelúdio: `set -o pipefail` (sem `set -e`), locale UTF-8 testado pelo efeito (`__p='ç'; [ ${#__p} = 1 ] || tentar C.UTF-8, C.utf8, en_US.UTF-8`), `IFS` padrão, `umask` intocado
- [x] Runtime sob demanda: `runtime/bash/*.sh` com marcadores `# @fn nome` / `# @needs a b` / `# @target linux|macos|all`; `include_str!` + resolução transitiva; **ordem determinística**; prefixo `__` em tudo
- [x] Parâmetros: `while/case` com `--x v`, `--x=v`, `--bool`, `--bool=false`, `-h/--help` (imprime `program.usage`), `--dry-run`, `--verbose`, `--` e posicionais em `args=()`; validação de `int`, `duration` (mesma gramática dos literais), enum (lista os casos), listas `a,b,c`; obrigatórios; `secret` de `NALU_X`; erros com código 2 + `use --help para ver as opções`
- [x] Checagem de dependências (`command -v`), código 127, pulada no dry-run
- [x] Variáveis: escalares em variáveis; `local` em funções; `bool` como `true`/`false` textuais; `duration` em ms inteiros; listas em arrays; **structs achatados** (`v__campo`; campos lista/mapa com codificação própria); listas de structs: um array por campo; **mapas**: `declare -A` + array de chaves para ordem (Linux) / dois arrays paralelos com busca (macOS; ≥ 16 chaves use um índice ou aceite O(n) e **documente o custo**)
- [x] Cópia por valor em `Let`/`Assign`/passagem/retorno de coleções e structs
- [x] `Expr`: aritmética em `$(( ))` (64 bits, truncamento como C), comparação de texto **ordinal** (`[[ a < b ]]` com `LC_ALL=C` num helper), `Concat`/`Interp` em `"…${x}…"`, `PathJoin`, `Len` (`${#x}` chars em UTF-8; `${#xs[@]}`), `Index`, `Field`, `And/Or/Not`
- [x] `Rvalue::Call` para **toda** entrada `v1` de `STDLIB`, `METHODS`, `PATH_PROPS`, `BUILTINS` (um teste confere a cobertura lendo as tabelas); funções do usuário com `__ret`/`__ret_a`/`__ret__campo`, listas e structs passados como "n itens"; `Lambda` em `filter/map/any/all/find/sort_by` como funções geradas `__lambda_N`
- [x] `OnFail` nas cinco formas, em `Call`, `Capture`, `Command`, `Try`, `Retry`, `Parallel`; `CurrentErr` = `__err_msg`/`__err_code`; propagação por contexto (função → `return 1`; topo → `__fail`; `try` → função `__try_N` com código 0/1/2/3/4; `retry` → laço; `parallel` → coleta)
- [x] `Command`: cada `Word::Lit` entre aspas simples (`'\''`), `Interp` em aspas duplas, `Spread` como `"${xs[@]}"`; pipeline com `pipefail`; redirecionamentos; `stdin` via `printf '%s' "$v" |`; `--verbose` (`+ cmd…` com segredos `******`); `--dry-run` (`[dry-run] cmd…`, não executa, exceto `Capture`/`CommandOk`); mensagem de falha `o comando \`git push\` falhou com código 1` / `não foi encontrado` (127); **dependências** no topo
- [x] `If`, `While`, `ForRange` (`for ((…))`), `ForList` (cópia do item), `ForMap` (ordem de inserção), `Match` (`case` para literais; `if/elif` com faixas; enum = texto), `Break/Continue/Return/Exit`
- [x] `Defer`: pilha **global** `__defers` com marca por função (`exit()` dentro de função roda tudo), `trap __run_defers EXIT`, `INT`→130, `TERM`→143, `PIPE`→141; falha dentro do defer ignorada
- [x] `Retry`: laço com `sleep` (ms → `sleep 0.5` funciona no BSD e GNU)
- [x] `Parallel`: subshells `&`; Linux `wait -n` para o `max`; macOS `wait $pid` por slot; resultados/erros em `mktemp -d`, lidos na ordem; erro agregado `parallel: e1; e2`; `collect` para a lista
- [x] `Native { bash }` inserido literal
- [x] `fs.*` com guardas (`rm -rf --` recusa `"" / . .. ~ $HOME $PWD`); `fs.read` remove BOM; `fs.write` sem BOM; dry-run respeita temporários criados nesta execução (`__temps` + prefixo)
- [x] `log.*` no stderr com prefixo; `print` com `printf '%s\n'`; `ui.*` com `/dev/tty` quando houver
- [x] `http.*` via `curl` (ou `wget`); `time.*` com `date` GNU × BSD; `sys.*`; `proc.*`; `dirs.*` (XDG × `~/Library`)
- [x] `Mode::Tests`
- [x] `duration_str` (`1m30s`, `250ms`, `0s`, negativo), `to_int` com faixa de 64 bits (recusa > 19 dígitos / fora da faixa), `str` methods por caractere (UTF-8)

## Fora de escopo

PowerShell, CLI, parser. `regex`, `hash`, `encoding`, `archive`, `pkg` (v0.2: emita `E090`? Não: o backend não emite diagnósticos; gere uma chamada a `__unimplemented 'regex.test'` que falha em runtime com mensagem clara, e anote no relatório).

## Critérios de aceite

1. `cargo test` passa, incluindo um teste que **gera** o script para IR feita à mão (use `ir::Program::empty` + construtores) para cada construção e roda com `/bin/bash` (3.2), comparando a saída.
2. Todo `.sh` gerado passa em `bash -n` com `/bin/bash`. Com `shellcheck` disponível, zero avisos de severidade `error`.
3. `runtime/bash/*.sh` passa em `bash -n` com `/bin/bash` e cada helper tem ao menos um teste (um script em `runtime/bash/tests.sh` que roda com `/bin/bash` e com qualquer bash 4+ se existir).
4. Cobertura da stdlib: teste que percorre `STDLIB`/`METHODS`/`PATH_PROPS`/`BUILTINS` com `v1: true` e confere que o backend tem implementação (sem `todo!`).
5. **Benchmark**: um laço de 20.000 iterações com `+=` e interpolação, e um mapa com 2.000 chaves distintas e 4.000 `set`/`get`, rodam em menos tempo que os equivalentes da Stela nesta máquina (Stela: `mapas_grandes` em 0,16 s no Bash 3.2). Registre os números no Relatório.
6. Script gerado para `examples/ola.nalu` com ≤ 100 linhas e sem helpers não usados.

## Testes

- `src/bash/tests.rs` (ou `#[cfg(test)]` em `mod.rs`): IR à mão → script → executa em `/bin/bash` → compara.
- `runtime/bash/tests.sh`.
- Quando o frontend (SPEC-001) tiver `nalu emit --ir`, teste também com os `examples/`.

## Notas de implementação

- Abstração sugerida: um `enum Val { Lit(String), Var(String), Dq(String), Arith(String), Array(String) }` com `fn word(&self) -> String` (como citar para argumento), `fn dq(&self)` (dentro de `"…"`), `fn arith(&self)`. Centralize **todo** quoting em 3-4 funções: `sq(s)`, `dq_escape(s)`, `word(val)`.
- Nunca `eval` com dado do usuário. Para nomes dinâmicos de variáveis (structs em listas, mapas no 3.2), `printf -v "$name"` e `${!name}` bastam.
- `__ret` para escalar; `__ret_a=()` para lista; structs: `__ret__campo` por campo; listas de structs: `__ret_a__campo`. Funções recebem listas como `n item1 … itemn`; structs como um argumento por campo (listas dentro de struct codificadas com `\x1f`).
- Comparação de texto: `__str_lt() { local LC_ALL=C; [[ $1 < $2 ]]; }` (o Bash refaz `setlocale` com `local LC_ALL`).
- `len` de string em caracteres: `${#s}` com locale UTF-8 ativo (por isso o prelúdio garante o locale).
- `sort` de listas: `printf '%s\0' … | LC_ALL=C sort -z` + `read -d ''` (funciona no 3.2, GNU, BSD, BusyBox). Para `sort_by`, "chave\tíndice" com `-z`.
- `duration`: ms inteiros; `sleep` aceita `0.5` em todos os alvos; `time.now_ms`: `date +%s%N` no GNU (BusyBox também), `python3`/`perl` **não**: no macOS use `date +%s` × 1000 e documente a precisão (ou `$EPOCHREALTIME` no Bash 5 quando houver).
- `fs.walk`: `find "$d" -type f -print0 | LC_ALL=C sort -z`, caminhos relativos a `$d` como passado.
- Erros de comando: `__err_msg="o comando \`$prog\` falhou com código $rc"`; 127 → `não foi encontrado`.
- `--verbose`/`--dry-run` mostram `Command.text` com `{…}` substituídos; `Secret` → `******`.
- Mantenha o gerador em métodos pequenos (`gen_for_range`, `gen_match`, `gen_call_std`…): o revisor vai ler.
- Referências úteis: BashFAQ/105 (por que não `set -e`), ShellCheck SC2086/SC2016, Bash 3.2 manual (sem `${x,,}`).

## Relatório

### O que existe

- `src/bash/`: `mod.rs` (`generate`), `gen.rs` (quadros, montagem, instruções, `try`/`retry`/`defer`/testes), `expr.rs` (palavras, aritmética, condições, onde moram as coleções), `store.rs` (cópia por valor, literais, mapas), `call.rs` (usuário, builtins, stdlib, métodos, lambdas, `parallel`), `cmd.rs` (`$`, `$( )`, `if $`, verbose/dry-run), `params.rs`, `quote.rs` (todo o quoting: `sq`, `dq_esc`, `W`), `runtime.rs` (helpers sob demanda), `val.rs`, `tests.rs`.
- `runtime/bash/`: `core.sh`, `str.sh`, `list.sh`, `map.sh`, `fs.sh`, `sys.sh` (116 blocos `# @fn`, com `# @needs`/`# @target`) e `tests.sh` (186 verificações, **todo helper testado**; o próprio script acusa helper sem teste).
- Testes: `cargo test --lib bash::` = 30 testes (IR à mão → script → `/bin/bash` 3.2 → compara saída/código), mais `cobertura_da_stdlib` (percorre `STDLIB`/`METHODS`/`PATH_PROPS`/`BUILTINS` `v1` nos dois alvos e falha se aparecer `__unimplemented`), `runtime_testes_de_unidade` (roda `tests.sh`; roda também o alvo `linux` se houver Bash 4+ no PATH) e `exemplos_geram_bash_valido` (frontend real → `bash -n` nos dois alvos, sem helper sem uso, `ola.nalu` ≤ 100 linhas). O e2e do agente C (`casos_no_bash`, 41 casos; `nalu_test_pacote_com_falha`; `nalu test examples`) passa.
- Exemplos executados de verdade no `/bin/bash` 3.2: `ola` (saída e `--help`), `backup` (pasta temporária, `parallel(max: 4)` com resultado, erro de uma pasta tratado com `or { }`, `defer`), `deploy --dry-run` (segredo por `NALU_TOKEN`, enum inválido → código 2, dependências → 127), `release` num repositório git temporário (tag criada, "nenhum commit", "mudanças não commitadas"). Ctrl+C (SIGINT no grupo) roda os `defer` e sai com 130; SIGTERM com 143.

### Benchmark (critério 5), nesta máquina, Bash 3.2, tempo de CPU médio de 20 execuções

| Programa | Stela | Nalu |
|---|---|---|
| laço de 20.000 iterações com `+=` e interpolação | 0,144 s | 0,145 s (empate: os dois geram o mesmo laço mínimo `for (( ))` + 2 atribuições; não há o que tirar) |
| mapa com 2.000 chaves, 2.000 `set` + 2.000 `get` | 1,010 s | **0,231 s** (4,4× mais rápido) |

Os equivalentes da Stela foram escritos em `.stela` e emitidos com `stela emit --target darwin`; o `mapas_grandes.stela` de referência roda em 0,16 s. O tempo de parede oscila ±30% nesta máquina (outros agentes rodando); medi CPU (`times` dos filhos). Para reproduzir: `cargo test --lib bash::tests::benchmark -- --nocapture` grava os scripts e imprime o tempo.

O que fez diferença: atribuição de uma expansão sozinha sem aspas (`n=$(( n + i ))` é ~10% mais rápido que `n="$(( n + i ))"` no 3.2), `x = x + y` inteiro vira `(( x += y ))`, e no mapa do 3.2 a codificação da chave fica dentro de `__map_pos` (um nível de função a menos por operação).

### Decisões

- **Mapas no Bash 3.2**: `m__k`/`m__v` (arrays densos na ordem de inserção) + **uma variável por chave** `__m<id>_<chave codificada>` com a posição, lida com `${!nome}` e escrita com `printf -v` (sem `eval`). Busca O(1) em qualquer tamanho (não há o limiar de 16 chaves da Stela). A codificação é injetiva: letras e dígitos ficam, todo outro byte vira `_hh` (`_` → `_5f`); o caminho comum (`a-z0-9_-./` e espaço) usa só substituições `${//}`; o resto vai byte a byte com `LC_ALL=C`. `m__id` vem de um contador global; `m__n` guarda os nomes para apagar o índice com um `unset "${m__n[@]}"`. Funções com mapas locais (ou parâmetros/temporários de mapa) ganham um *wrapper* que apaga o índice ao sair; reatribuir um mapa reaproveita o `id` e apaga o índice antigo. `remove` compacta os arrays (O(n), raro). Linux: os mesmos `m__k`/`m__v` + `declare -A m__i` (chave prefixada com `_`, para a chave vazia), então iteração, `keys`, `values`, cópia e passagem são o mesmo código nos dois alvos.
- **`try` e `retry` viram laços `while :`** em vez de funções `__try_N`: falha dentro do bloco = `break N`/`continue N` (N conta os laços do usuário no meio), `return`/`break`/`continue` do usuário funcionam direto, e as variáveis locais continuam visíveis. Mais barato e mais legível; a semântica é a da spec.
- **Escopo dinâmico**: variáveis locais de função com o mesmo nome de uma do topo ganham o prefixo `__l_` (senão uma função chamada leria a local da outra). Temporários do lowering (`__t1`) não precisam.
- **`defer`**: pilha global `__defers` (palavras + quantidade), um `__defer_N` gerado por `defer` com os argumentos avaliados na hora; funções com `defer` (ou `sys.cd`, ou mapas locais no macOS) ganham *wrapper* `fn_x` → `fn_x__corpo` que roda os `defer` até a marca e devolve o código. `__run_defers` protege `__ret`/`__ret_a`/`__err_*`.
- **Texto dos comandos** (`--verbose`, `--dry-run`, mensagens): `Command.text` com cada `{…}` (fora de aspas simples) trocado pelo valor, segredos como `******`, listas separadas por espaço; se a contagem de `{…}` não bater com as expressões, remonta a partir das palavras. Falha: ``o comando `<texto>` falhou com código N``; 127: ``o comando `<programa>` não foi encontrado``.
- `OnFail::Fail` sem `code` usa código 1 (como `fail`), não o código original.
- `lista[i]` fora da faixa: vazio para texto, `0` para `int`/`duration`, `false` para `bool` (pedido do corpus); índice negativo também (guarda `i < 0 ? 1<<62 : i`).
- `ui.*` consideram "terminal" quando a entrada padrão é um terminal (`[[ -t 0 ]]`), não `/dev/tty`: assim scripts em CI e nos testes nunca ficam esperando.
- Modo testes: sem linha em branco antes do resumo, para bater com `tests/pacote/saida.txt` (o texto da spec e da referência §23 fala em linha em branco: **um dos dois precisa ser corrigido**; segui o arquivo, como o PowerShell).
- Locale: o prelúdio testa `${#'ç'} == 1` e tenta `C.UTF-8`, `C.utf8`, `en_US.UTF-8`, `en_US.utf8`; ordem de texto sempre com `LC_ALL=C` local (`__str_lt`, `sort -z`, globs de `fs.ls`/`fs.glob`).

### Limitações e o que ficou de fora

- **Alvo Linux não foi executado**: esta máquina só tem Bash 3.2 (sem Docker/Homebrew). Os scripts Linux passam em `bash -n` e o código específico (`declare -A`, `${x^^}`, `printf '%(…)T'`, `wait -n`, `stat -c`, `date -d`) foi revisado; `runtime/bash/tests.sh linux` está pronto e o teste Rust o roda automaticamente onde houver Bash 4+. É o primeiro item para o CI (Ubuntu e Alpine/BusyBox).
- `shellcheck` não está instalado: não rodei.
- `regex`, `hash`, `encoding`, `archive`, `pkg` (v0.2) geram `__unimplemented 'regex.test'`: falha em tempo de execução com ``erro: regex.test ainda não está implementado no alvo Bash (v0.2)`` e código 70.
- `time.now_ms`/`time.since_ms` no macOS têm precisão de segundos (o `date` BSD não tem `%N`; sem Python/Perl). No Linux: `$EPOCHREALTIME` (Bash 5) ou `date +%s%N`.
- Bug do Bash 3.2: `"${@:i:n}"` junta os itens com espaço quando `IFS` não contém espaço. O script nunca muda `IFS`, mas um `native` que mude `IFS` globalmente quebra a passagem de listas para funções.
- Itens de lista de structs com campo lista/mapa são codificados com `\x1f` (um item contendo `\x1f` se divide, como na Stela).
- `exit()` dentro de uma tarefa de `parallel` encerra só a tarefa (que aparece como "terminou sem resultado"); `max` padrão = `sys.cpus()`.
- Mapas temporários decodificados no **topo** (campo mapa de item de lista de structs) não têm o índice apagado (vazamento pequeno, só no macOS).
- `path / "/abs"` concatena (`a//abs`); `fs.walk` inclui links para arquivo.

### Pedidos de mudança nos contratos

Nenhum obrigatório. Sugestões: (1) alinhar o texto da SPEC-002/§23 com `tests/pacote/saida.txt` (linha em branco antes do resumo); (2) documentar em §18 as mensagens de erro de uso que o Bash emite (`o parâmetro --x é obrigatório`, `--x precisa ser um número inteiro (recebeu '…')`, `--x precisa ser um de: a, b (recebeu '…')`, `falta o valor de --x`, `opção desconhecida: --y`) para o PowerShell usar as mesmas; (3) `OnFail::Fail` sem `code`: confirmar "código 1".
