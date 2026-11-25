# Fuzzers del frontend y de los backends

Scripts de Python 3 (solo biblioteca estándar). Usan `target/release/yerux` (o `$YERUX`) y los programas de `tests/` y `examples/` como semillas.
Cada caso fallido queda en `<out>/c<N>/` (`.yx` incluido) y se imprime una línea por hallazgo; al final `done N ... bad K`.

- `fuzz_syntax.py SEED N OUT` — entradas malformadas (truncar, duplicar o borrar tokens y llaves, Unicode raro, números enormes,
  anidamiento profundo, cambios de palabras clave y tipos). Pasa `yerux check` (plazo 5 s); con `DEEP=1` también `yerux c` e `ir`.
  Busca pánicos (`panicked at src/`, código >= 101, señal), desbordes de pila, cuelgues y errores sin texto.
- `fuzz_valid.py SEED N OUT` — mutaciones que conservan la sintaxis (literales, operadores, tipos, orden de sentencias, funciones
  duplicadas, closures, modos `view`/`mut`/`move`). Si compila: `check`, `c`, `ir`, build C y `--native`, y compara salida y código
  de salida de los dos ejecutables. `ASAN=1` añade una compilación C con `-fsanitize=address,undefined` (vía `YERUX_CC_FLAGS`).
  `J=n` fija los hilos (6 por defecto). La diferencia «C sin guarda de pila / nativo con `stack overflow`» está documentada y se ignora.

Ejemplo: `python3 tools/fuzz/fuzz_valid.py 7 2000 /tmp/fz-out` (unos 2-3 minutos con 6 hilos).
