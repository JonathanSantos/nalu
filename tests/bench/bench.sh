#!/bin/bash
# Roda os benchmarks Nalu × Stela no Bash desta máquina.
# Uso: tests/bench/bench.sh [alvo]   (alvo: macos|linux; padrão: macos)
set -u
cd "$(dirname "$0")/../.." || exit 1
alvo="${1:-macos}"
stela_alvo="$alvo"; [ "$alvo" = macos ] && stela_alvo=darwin
nalu="target/release/nalu"
stela="../mis/target/release/stela"
out="$(mktemp -d)"
printf '%-8s %-6s %10s %10s\n' caso lang tempo saida
for caso in laco mapa lista; do
    if [ -x "$nalu" ]; then
        "$nalu" build "tests/bench/$caso.nalu" --target "$alvo" -o "$out/nalu" >/dev/null 2>&1 || echo "nalu: falhou ao compilar $caso"
        f="$out/nalu/$caso.$alvo.sh"
        if [ -f "$f" ]; then
            s=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
            r=$(bash "$f" 2>&1 | tail -1)
            e=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
            printf '%-8s %-6s %10.3fs %s\n' "$caso" nalu "$(echo "$e - $s" | bc)" "$r"
        fi
    fi
    if [ -x "$stela" ]; then
        "$stela" build "tests/bench/$caso.stela" --target "$stela_alvo" -o "$out/stela" >/dev/null 2>&1 || echo "stela: falhou ao compilar $caso"
        f="$out/stela/$caso.$stela_alvo.sh"
        if [ -f "$f" ]; then
            s=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
            r=$(bash "$f" 2>&1 | tail -1)
            e=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
            printf '%-8s %-6s %10.3fs %s\n' "$caso" stela "$(echo "$e - $s" | bc)" "$r"
        fi
    fi
done
rm -rf "$out"
