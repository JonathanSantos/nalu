# SPEC-001 — Frontend: lexer, parser, checker e lowering

| Campo | |
|---|---|
| Dono | agente A |
| Status | 🟢 entregue (aguarda aceite do orquestrador) |
| Depende de | `src/ir.rs`, `src/diag.rs`, `src/stdlib.rs`, `src/target.rs` (congelados); [02-linguagem.md](../02-linguagem.md) é a spec da linguagem |
| Arquivos | `src/lexer.rs`, `src/parser.rs`, `src/ast.rs`, `src/check.rs`, `src/lower.rs`, `tests/errors/*.nalu` |

## Objetivo

`nalu::compile(source, file, embed_root, target)` transforma qualquer `.nalu` válido na `ir::Program` do alvo, e qualquer `.nalu` inválido numa lista de diagnósticos com código, posição e ajuda, **sem nunca entrar em pânico**.

## Contrato

Produz (em `src/lower.rs`):

```rust
pub fn compile(source: &str, file: &str, embed_root: &std::path::Path, target: Target) -> crate::Compiled;
// Compiled { program: Some(ir) se não houve erro (avisos podem existir), diags: todos, ordenados por posição }
```

Consome: `ir::*` (produz exatamente o que está documentado lá), `stdlib::{STDLIB, METHODS, PATH_PROPS, BUILTINS, RESERVED}` para resolver nomes/tipos/falibilidade, `diag::Diag`.

A IR tem **regras** (topo de `ir.rs`) que o lowering **garante**; os backends confiam nelas:
- `Expr` pura e escalar (coleções só `Var`/`Field`); toda chamada/captura/comando/coleção é `Rvalue` ou `Stmt`;
- `OnFail` em tudo que pode falhar; `Propagate` quando o usuário não escreveu `or`;
- nomes únicos por função (renomeie sombras: `x`, `x_2`; temporários `__t1`…); variáveis do topo podem ser referenciadas dentro de funções;
- `os` resolvido (ramo morto removido; `match os` reduzido ao braço certo); `embed` virou `ExprKind::Str`;
- `+=` → `Assign` + `Binary`; `else if` → `If` em `els`; argumentos nomeados/padrões preenchidos e posicionais; `if`/`match` como expressão → `Let` do temporário + `If`/`Match` com `Assign` em cada ramo; `while` com condição não pura → `While { cond: true }` + `If !c { Break }`; `&&`/`||` com lado direito não puro → `If` aninhado (curto-circuito preservado);
- `Interp` só com partes `Str/Path/Int/Bool/Enum`: duração → `Call Builtin duration_str`; `err` → `Field(CurrentErr, message)`;
- `Program.usage` pronto (formato exato em [02-linguagem.md §18](../02-linguagem.md#18-parâmetros-do-script)); `Program.deps` com os programas de nome fixo (sem builtins do shell: `cd echo test [ true false printf read exit export set unset`), na ordem de aparição;
- métodos de struct viram `Func { name: "Tipo.metodo", params: [self, …] }`; chamadas `s.metodo(a)` viram `Call User("Tipo.metodo") args [s, a]`;
- `Func.fallible` inferido: algum `Propagate` no corpo (fora de `try`/`retry`/`parallel` que o capturem) ou `Fail` fora de handler → `true`. Ponto fixo para recursão mútua.

## Escopo v0.1

Lexer
- [x] `NL` significativo (uma instrução por linha; continuação depois de operador, `,`, abertura de `([{` e antes de `.` no início da linha); comentários `//`, `/* */`, `///` (guardado como doc)
- [x] identificadores, palavras-chave, `int` (`1_000`, `0xFF`), `duration` (`500ms 30s 1.5s 5m 1m30s 2h 7d`, ordem maior→menor, só segundos com fração)
- [x] strings `"…{expr}…"` (partes re-lexadas com offset correto; `{{`/`}}` literais; escapes `\n \t \r \" \\ \{ \}`), `'cru'`, `"""` com dedent
- [x] linhas de comando: `$ palavra…` até `NL`, com palavras `Lit`, `{expr}`, `"…"`, `'…'`, misturas; `|`, `> >> 2> 2>> < 2>&1`; ` or ` sem aspas encerra; `{` sozinho no fim da linha encerra (condição); `$(…)` captura (parênteses balanceados)
- [x] operadores `+ - * / % == != < <= > >= && || ! = += -= *= /= %= .. ..= => -> . , : | ( ) [ ] { }`

Parser (resiliente)
- [x] tudo da gramática em [02-linguagem.md §27](../02-linguagem.md#27-palavras-chave-e-gramática)
- [x] recuperação: ao errar, emite `E005`, avança até `NL` ou `}` e continua; "todo item consome ≥ 1 token"; nunca loop infinito
- [x] `E006` com sugestão para `const`, `func`, `switch`/`case`, `throw`, `throws`, `try x` (prefixo), `??`, `++`, `for … of`, `import`, `null`, `=>` fora de lugar, `? :` ternário, `print "x"` sem parênteses

Checker
- [x] escopos, `let` × `var` (`E002`), redeclaração (`E003`), reservados (`E004`), nome inexistente com sugestão por distância de edição (`E001`), sombreamento permitido em funções
- [x] tipos: inferência, anotações, literais vazios exigem tipo, `str`↔`path`, `==` estrito (`E011`), aritmética, concatenação, unidades (`E020`, `E021`), `path /` (`E043`)
- [x] chamadas: stdlib (tabela), métodos (tabela, com `T/U/K/V`), funções do usuário, argumentos nomeados e padrões (`E012`), lambdas só como argumento (`E033`) e só leitura
- [x] **falibilidade**: marca cada chamada/comando/captura/índice de mapa; `or` só sobre falível (`E040`); `or ignore` só em `Unit`; `try` sem `or` (`E041`); `err`/`self`/`break`/`continue`/`return` fora de lugar (`E046`); inferência de `fallible` por função (ponto fixo)
- [x] `match`: padrões por tipo, exaustividade em enum e em `os` (`E031`), `_`; como expressão: mesmo tipo em todos os braços
- [x] `if` como expressão exige `else` e mesmo tipo; função com retorno termina em `return`/`fail` (`E032`); expressão sem efeito (`E030`)
- [x] structs: literais com e sem nome (`E044`), campos obrigatórios/padrão, `..base`, métodos e `self`, campos escalares/listas/mapas de escalares; enums e `Tipo.parse`; mapas (`m[k]` falível; `m[k] = v` só em `var`)
- [x] `param`: só no começo, tipos permitidos, `///`, reservados `dry_run/verbose/help`; `secret` (`E050`, `W011`)
- [x] `os` (`E051`), `native` (`E052`), `embed` (`E053`, lê o arquivo relativo a `embed_root`), `parallel` (`E045`), `retry`, `defer` (chamada ou comando simples), `test`/`assert`/`assert_fails` (`E060`)
- [x] avisos: `W001` variável não usada (exceto `_x`), `W002` param não usado, `W010` programa inexistente no alvo (lista mínima: `grep sed awk head tail cut tr wc sort uniq xargs find chmod chown ln ps kill sudo brew apt-get apt dnf yum pacman apk` × `winget choco scoop`), `W012` `or ignore` em `rm`/`mv`/`del`
- [x] mensagens em português, com `ajuda:` sempre que houver forma certa; tabela de mensagens num só lugar

Lowering
- [x] tudo da seção Contrato acima; `--explain` (lista `fn nome: pode falhar por: …`) exposto como `pub fn explain(program_ast) -> String` ou via diags informativos (decida e documente)
- [x] `usage` idêntico ao exemplo de [02-linguagem.md §18](../02-linguagem.md#18-parâmetros-do-script) (alinhamento por coluna, `(padrão: x)`, `(obrigatório)`, `(obrigatório; ou NALU_X)` para secret, flags padrão no fim)

## Fora de escopo

Gerar shell (SPEC-002/003), CLI (SPEC-004), `regex`, módulos do usuário, `fmt` do código.

## Critérios de aceite

1. `cargo test` passa; nenhum `unwrap`/`panic` alcançável por entrada do usuário (fuzz manual: arquivos truncados, `{` sem fechar, string sem fechar, bytes inválidos).
2. Os 5 arquivos de `examples/` compilam sem erro para os três alvos: `nalu emit examples/deploy.nalu --target windows --ir` mostra uma IR que respeita todas as regras (o orquestrador vai conferir à mão: nenhuma `Expr` com chamada, todo `Call` com `OnFail`, nomes únicos).
3. `tests/errors/` tem **ao menos 40** casos (um por código E/W da tabela, mais variações), cada um com `// error: E0xx` na primeira linha, e todos passam no harness (SPEC-004) ou num teste próprio em `check.rs` enquanto o harness não existe.
4. Um arquivo com 6 erros de sintaxe em linhas diferentes gera 6 diagnósticos numa passada.
5. `compile` de um arquivo de 1.000 linhas leva < 50 ms em release.

## Testes

- Unitários em cada módulo (`#[cfg(test)]`): lexer (strings com `{ }`, durações, comandos, `or`, `{` final), parser (cada construção; recuperação), checker (cada código), lowering (snapshots `{:#?}` de trechos pequenos).
- `tests/errors/*.nalu`.

## Notas de implementação

- Comece pelo **caminho feliz de `examples/ola.nalu`** de ponta a ponta (lexer→IR) e deixe `nalu emit --ir` mostrando a IR o mais cedo possível: os backends dependem disso para testar. Depois amplie.
- Interpolação: guarde o texto bruto de `{…}` no lexer com o offset e re-lexe/parse ao montar a AST, para erros dentro de `{ }` apontarem a coluna certa.
- O lexer de comandos é um modo próprio: entra em `$`, sai em `NL`, em ` or ` fora de aspas, em `{` que é o último token não-branco da linha, ou no `)` que fecha `$(`. Dentro de `{expr}` numa palavra, conte chaves aninhadas (mapas/structs dentro de interpolação são raros, mas `{xs.map(x => x)}` acontece).
- `Ty` para genéricos da tabela: resolva `T`, `U`, `K`, `V` pelo receptor; `(T) -> bool` indica lambda.
- `fallible` por função: itere até não mudar (chamadas a funções do usuário ainda não resolvidas contam como falíveis até prova em contrário? **Não**: comece com `false` para todas, propague até ponto fixo; recursão direta não é falível por si).
- Renomeação: mantenha um mapa `nome→nome_único` por escopo; o backend recebe `nome_2` já pronto. Nunca gere nomes começando com `__` para variáveis do usuário (reservado aos temporários/runtime).
- Mensagens: `E001 "o nome \`foo\` não existe"` + `ajuda: você quis dizer \`fo\`?` (distância ≤ 2, mesmo escopo + stdlib + módulos).
- Erros de tipo mostram os dois tipos com `Ty::display()`.

## Relatório

**Estado:** todo o escopo v0.1 implementado. `cargo test --lib`: 43 testes do frontend (lexer 12, parser 5, check 6, lower 20 — vários com dezenas de sub-casos), mais `tests/errors/` com **75 casos** (todos os códigos E/W da §25, com variações), conferidos tanto pelo harness (`erros_de_compilacao`) quanto por um teste próprio (`lower::tests::corpus_de_erros`).

**O que existe**
- `lexer.rs`: `NL` significativo com continuação (operadores, `,`, `([{`, `.` no começo da linha), `///` como doc do próximo token, durações (ordem maior→menor, fração só em `s`, até ms), `int` com `_`/hex e estouro reportado, strings com partes `{expr}` guardadas com o deslocamento do arquivo (erros dentro de `{ }` apontam a coluna certa), `{}` vazio literal (modelos de `fmt`), `"""` com dedent; modo comando (`$`, `$(`), com `|`, redirecionamentos, ` or `, `{` que fecha a linha, bloco `{ … }` de uma linha em condição (`while !$ nc … { time.sleep(1s) }`, exemplo da §13), e `)` sem par quando o comando está dentro de parênteses (`assert_fails($ false)`).
- `parser.rs`: descida recursiva da §27, recuperação por linha/`}` (6 erros em 6 linhas → 6 diagnósticos; teste), profundidade limitada, `E006` para `const func function def switch case throw throws try-prefixo ?? ++ -- for…of import null ?: print "x" => fora de lugar elif ;`.
- `check.rs`: árvore tipada (`TProgram`) com `VarId`s, todos os códigos E001–E090/W001–W012 (W010 fica no lowering, por alvo), sugestão por distância de edição (≤ 2), assinaturas da stdlib lidas das tabelas (genéricos `T/U/K/V`, `Enum`, lambdas, variádicos, padrões em texto), argumentos nomeados/padrões preenchidos, falibilidade com **ponto fixo** e conferências que dependem dela (`E040`, `E060`) depois da inferência; marcador de tipo desconhecido para não gerar cascatas. Mensagens frequentes no módulo `check::msg`.
- `lower.rs`: `compile` (roda numa thread com pilha de 256 MB: nem entradas absurdas estouram), `program_to_string`, `explain`, `format_duration`; IR A-normal com `OnFail` em tudo, nomes únicos (`x_2`, `__tN`), `os` resolvido (dobra `os == "…"`, `!`, `&&`, `||`; `match os` reduzido), `usage` §18, `deps` em ordem de aparição (sem builtins do shell nem caminhos `./x`), `W010` só no código vivo do alvo. Teste com um **validador da IR** (coleções só `Var`/`Field`, partes de `Interp`, nomes únicos por função, locais nunca com nome de global) rodando nos 4 exemplos × 3 alvos, em 31 programas válidos variados e em ~25 000 mutações aleatórias dos exemplos (fuzz: nenhum pânico, toda IR produzida é válida).
- Desempenho: arquivo de 1 037 linhas → `nalu check` (3 alvos) em ~11 ms em release (~3,5 ms por alvo).

**Decisões**
1. Com erro de sintaxe, o checker não roda (evita cascata de `E001`); só os erros do parser são mostrados.
2. `or` cujo lado esquerdo tem partes falíveis aninhadas (`f(g()) or 0`) vira `Let slot` + `Try { … } or …`, para o `or` cobrir a expressão toda; `or` direto (`OnFail` na própria chamada/captura/comando) quando não há nada falível aninhado. `or fail "…{f()}"` com mensagem impura vira `OnFail::Block { err: Some(nome) }` + `Fail` (nunca usa `CurrentErr` depois de outras instruções).
3. `if`/`match`/`parallel` como valor: `Let __tN` mutável com o "zero" do tipo (structs com campos zero, listas vazias; `secret` via `Builtin secret("")`) + `Assign` nos ramos; depois `Let x = Var(__tN)`.
4. Item de lista de structs usado como valor (`let s = xs[0]`, `xs[0].metodo()`) é copiado campo a campo num `Rvalue::Struct` (regra 6); como base de campo (`xs[1].nome`) fica `Field(Index(…))`.
5. Funções e testes nunca reutilizam nomes de **globais diretas** (params e `let`/`var` do topo): evita o escopo dinâmico do Bash enxergar uma local de quem chamou. `self` é o primeiro parâmetro dos métodos, com esse nome.
6. `usage`: a 1ª linha termina **sempre** com `[opções] [args…]` (§18 e `tests/cases/params_help`); por isso difere do `Program::empty` de `ir.rs`, que não tem `[args…]`. `bool` com padrão `true` mostra `(padrão: true)`; padrões de duração com `format_duration` (`0s`, `250ms`, `1.5s`, `1m30s`, `1d2h`; misto com ms → `1m500ms`).
7. `"""`: a linha do `"""` final (só espaços) não entra; sem `\n` final (como Swift; dúvida 1 do `tests/cases/README.md`).
8. `--explain`: `lower::explain(source, file, embed_root) -> String` (`fn nome: pode falhar por: \`fs.read\` (linha 12), …` / `fn nome: não falha`), a partir das razões do checker (inclui `fail`, `or fail`, comandos, chamadas de funções falíveis). Hoje o CLI usa a sua própria leitura da IR; pode trocar por esta.
9. Lambdas com algo falível dentro deixam a chamada do método falível (`Call.fallible = true`, mesmo que a tabela diga que o método não falha). `break`/`continue` não atravessam `retry`, `parallel` nem lambdas (`E046`); `return` dentro de `retry` é permitido, dentro de `parallel` é `E045`.
10. `param` só aceita padrão fixo (literal, caso de enum, lista de literais); `secret` com padrão é `E050`. Listas de listas, mapas de coleções, struct em struct e lista de structs como campo: `E090` (v0.2).
11. `defer` aceita chamada (qualquer) ou comando simples (1 estágio, sem redirecionamento/stdin/`or`).

**O que não funciona / falta**
- A tabela de mensagens está centralizada só em parte (`check::msg`); mensagens do lexer/parser continuam no ponto de uso.
- Os 4 casos de `tests/cases` que ainda falham no Bash (`fs` glob com `.`/`..`, `listas` `xs[99]` → `0`, `numeros` `"+5".to_int()`, `path` `~` em `.parent`) e o resumo do `tests/pacote` são de runtime/CLI; conferi a IR dessas linhas e está correta. No PowerShell todos os casos passam.

**Contratos:** nenhuma mudança em `ir.rs`/`diag.rs`/`stdlib.rs`/`target.rs`. Observação: `Program::empty().usage` não traz `[args…]`; se o orquestrador quiser os dois iguais, é ajustar o helper (mudança só de texto, não de tipo).
