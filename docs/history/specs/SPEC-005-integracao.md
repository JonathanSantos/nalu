# SPEC-005 — Integração, benchmarks e comparativo

| Campo | |
|---|---|
| Dono | orquestrador |
| Status | 🟢 concluída (27/10/2026) |
| Depende de | SPEC-001 a SPEC-004 |
| Arquivos | `tests/bench/`, `docs/05-vs-stela.md`, `README.md`, correções pontuais em qualquer módulo |

## Objetivo

Os quatro `examples/` compilam, rodam no Bash 3.2 desta máquina e os casos do corpus passam; números de sintaxe e performance registrados contra a Stela.

## Plano

1. `cargo build` + `cargo test` com tudo junto; triagem das falhas por spec; correções pequenas aqui, grandes de volta ao dono.
2. `nalu emit --ir` dos exemplos: revisão manual das regras da IR.
3. Oráculo diferencial: todo caso com `.out` passa no Bash; `.ps1` validado quando houver pwsh.
4. Benchmarks em `tests/bench/`: laço 20k, mapa 2k chaves, lista 10k com `filter/map/sort`, 200 chamadas de função com struct; Nalu × Stela, Bash 3.2 (e Bash 5 se disponível).
5. Contagem de tokens/linhas dos exemplos portados; atualizar `05-vs-stela.md` com os números.
6. README do repositório; `CHANGELOG-ir.md` fechado para v0.1.

## Critérios de aceite

- `cargo test` verde nesta máquina.
- `nalu run examples/ola.nalu -- --nome Ana --vezes 2` imprime o esperado.
- `nalu test examples` passa 6/6.
- Tabela de números em `05-vs-stela.md` preenchida.

## Relatório

- `cargo test` com `NALU_PWSH` apontando para o pwsh 7.6.9: 102 testes de biblioteca, 6 da CLI, 10 e2e (41 casos do corpus no Bash 3.2 **e** no pwsh, 75 casos de erro, 4 exemplos nos 3 alvos, `nalu test examples` 6/6, `tests/pacote`). Zero avisos de compilação. Binário release: 1,3 MB.
- Revisão da IR de `backup.nalu`: toda chamada/comando com `OnFail`, nenhuma chamada dentro de `Expr`, nomes únicos.
- **Regressão achada e corrigida na integração:** o benchmark `mapa` levava 9,4 s (Stela: 1,5 s). Causa: o Bash 3.2 copia `$@` a cada chamada de função aninhada, então um laço que chama helpers enquanto `$@` tem milhares de itens é quadrático. Duas correções em `src/bash/gen.rs` e `runtime/bash/map.sh`: parâmetros de lista são copiados no wrapper e o corpo é chamado sem argumentos (`set --` nas funções sem wrapper); `__map_index` lê as chaves de um array global. Resultado: 0,28 s.
- Benchmarks finais e contagem de tokens: em [05-vs-stela.md](../05-vs-stela.md).
- Tamanho do gerado: `ola` 96 linhas (macOS), 100 (Linux), 245 (PowerShell); `deploy` 327 / 327 / 635.
- Compilação de `deploy.nalu` para os 3 alvos: 6 ms.
- CI escrito em `.github/workflows/ci.yml` (Ubuntu, macOS, Windows 5.1, Alpine); **nunca rodou**: a pasta não é um repositório git.
- Não verificado: alvo `linux` em Bash 4+ (só `bash -n`); Windows PowerShell 5.1 real (só pwsh 7 no macOS); `shellcheck`.
