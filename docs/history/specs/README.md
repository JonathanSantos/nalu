# Specs: as frentes de trabalho

Cada spec é **uma tarefa com dono**, escrita para que alguém (pessoa ou agente) a execute sozinho, em paralelo com as outras, sem precisar perguntar. O que liga as frentes são os **contratos congelados**: [`src/ir.rs`](../../src/ir.rs), [`src/diag.rs`](../../src/diag.rs), [`src/stdlib.rs`](../../src/stdlib.rs), [`src/target.rs`](../../src/target.rs) e as assinaturas públicas em [04-arquitetura.md](../04-arquitetura.md). Quem precisar mudar um contrato faz a mudança **aditiva**, anota em [CHANGELOG-ir.md](CHANGELOG-ir.md) e avisa o orquestrador.

## Quadro

| Spec | Frente | Dono | Status | Arquivos |
|---|---|---|---|---|
| [SPEC-001](SPEC-001-frontend.md) | Frontend: lexer, parser, checker, lowering | agente A | 🟢 aceita | `src/lexer.rs` `src/parser.rs` `src/ast.rs` `src/check.rs` `src/lower.rs` `tests/errors/` |
| [SPEC-002](SPEC-002-backend-bash.md) | Backend Bash (Linux 4.3+, macOS 3.2) + runtime | agente B | 🟢 aceita (Linux só `bash -n`) | `src/bash/` `runtime/bash/` |
| [SPEC-003](SPEC-003-backend-powershell.md) | Backend PowerShell 5.1 + runtime + lançador | agente C | 🟢 aceita (pwsh 7; `parallel` sequencial) | `src/ps/` `runtime/ps/` |
| [SPEC-004](SPEC-004-cli-testes.md) | CLI, harness de testes, corpus de casos | agente C | 🟢 aceita | `src/main.rs` `tests/e2e.rs` `tests/cases/` `tests/pacote/` `tests/ps_syntax.ps1` |
| [SPEC-005](SPEC-005-integracao.md) | Integração, benchmarks, comparativo final | orquestrador | 🟢 concluída | `tests/bench/` `docs/05-vs-stela.md` |

Legenda: ⚪ não começou · 🟡 em andamento · 🟢 aceita · 🔴 bloqueada.

## Padrão de uma spec

```
# SPEC-NNN — Título

| Campo | |
|---|---|
| Dono | quem executa |
| Status | ⚪/🟡/🟢/🔴 |
| Depende de | contratos e outras specs |
| Arquivos | os que esta spec cria/edita (e **só** esses) |

## Objetivo         — uma frase: o que existe quando terminar
## Contrato         — o que consome e o que produz (assinaturas exatas)
## Escopo v0.1      — lista fechada do que entra (checkboxes)
## Fora de escopo   — o que NÃO fazer agora (para não invadir outra spec)
## Critérios de aceite — como o orquestrador vai conferir (comandos, saídas)
## Testes           — o que a spec entrega em testes
## Notas de implementação — dicas, armadilhas, referências
## Relatório        — preenchido pelo dono ao terminar: o que fez, o que faltou, decisões tomadas
```

## Regras de convivência

1. **Só edite os arquivos listados na sua spec.** Precisa de algo em outro arquivo? Anote no seu Relatório; o orquestrador integra.
2. `cargo build` tem que continuar passando a cada passo: os módulos começam como stubs que compilam.
3. Nada de dependências externas no `Cargo.toml`.
4. Teste no shell de verdade sempre que puder: `/bin/bash` (3.2) existe nesta máquina; `pwsh` 7.6.9 está disponível fora do PATH, em `/private/tmp/claude-503/-Users-jovivo-Developer-projects-langs-rival/402acefe-70af-4d3f-81de-b47b10f672b6/scratchpad/pwsh/pwsh`: exporte `NALU_PWSH` com esse caminho. É o PowerShell 7 no macOS, então a validação estática de sintaxe do 5.1 continua obrigatória.
5. Mensagens de diagnóstico em português, com o código da tabela em [02-linguagem.md §25](../02-linguagem.md#25-diagnósticos).
6. Ao terminar, preencha o **Relatório** no fim da sua spec e marque o status.
