# Fuzzers del frontend y de los backends

Scripts de Python 3 (solo biblioteca estándar). Usan `target/release/yerux` (o `$YERUX`). Los fuzzers por mutación toman sus semillas de `tests/` y `examples/`, guardan
cada fallo en `<out>/c<N>/` e imprimen el total en `done N ... bad K`.

- `fuzz_syntax.py SEED N OUT` — entradas malformadas (truncar, duplicar o borrar tokens y llaves, Unicode raro, números enormes,
  anidamiento profundo, cambios de palabras clave y tipos). Pasa `yerux check` (plazo 5 s); con `DEEP=1` también `yerux c`, `ir` y `fmt --stdout` (sin pánico; la salida compila y es punto fijo).
  Busca pánicos (`panicked at src/`, código >= 101, señal), desbordes de pila, cuelgues y errores sin texto.
- `fuzz_valid.py SEED N OUT` — mutaciones que conservan la sintaxis (literales, operadores, tipos, orden de sentencias, funciones
  duplicadas, closures, modos `view`/`mut`/`move`). Si compila: `check`, `c`, `ir`, build C y `--native`, y compara salida y código
  de salida de los dos ejecutables. `ASAN=1` añade una compilación C con `-fsanitize=address,undefined` (vía `YERUX_CC_FLAGS`).
  `J=n` fija los hilos (6 por defecto). La diferencia «C sin guarda de pila / nativo con `stack overflow`» está documentada y se ignora.

Ejemplo: `python3 tools/fuzz/fuzz_valid.py 7 2000 /tmp/fz-out` (unos 2-3 minutos con 6 hilos).

## C frente a nativo con programas generados

```sh
cargo build --release
python3 tools/fuzz/fuzz_backends.py --seed 231000 --count 2100 -j 8 --out /tmp/yerux-backends
```

Genera programas válidos desde cero con aritmética `Int` y `UInt8` a `UInt64` (división, desbordamientos, operaciones
modulares), Unicode, listas, mapas, structs, enums, closures y patrones sobre `Option` y `Result`. Yerux 1.0 no tiene
operadores de desplazamiento. Las pruebas cubren su equivalente sin signo con división y multiplicación modular por
potencias de dos.

Compara byte a byte stdout y el código de salida de `yerux run` y `yerux run --native`. Requiere un compilador C.
Guarda el programa, stdout, stderr y estado de ambos backends para cada diferencia, rechazo o timeout en
`<out>/<perfil>-<semilla>/`. Para repetir un caso usa `--profile PERFIL --seed SEMILLA --count 1` con otro directorio de
salida. Usa `--no-opt` para desactivar optimizaciones en ambos backends. Se ejecuta manualmente, fuera de la suite.

En `f1514ce` (Linux x86-64) coincidieron los 2100 casos del ejemplo y otros 210 con `--seed 240000 --count 210 --no-opt`.

- `fuzz_stdlib.py [--cases N] [--seed S] [--suite LIST] [--batch B] [--jobs J] [--backend c|native] [--out DIR]`
  — comparación diferencial de la biblioteca estándar contra oráculos de Python: `std.json` contra `json`,
  `std.csv` contra `csv`, `std.toml` contra `tomllib`, y `std.text`, `std.fmt` y `std.args` contra una
  implementación de la regla escrita en `docs/STDLIB.md`. Genera entradas aleatorias y malformadas, compila los
  casos en lotes (un programa por lote, backend nativo: unos 3000 casos en 20 s con `--jobs 6`) y compara cada
  respuesta; un caso que hace pánico o no compila se aísla y su programa queda en `<out>/caseNNNNN_<suite>.yx`
  (por defecto `/tmp/yerux_fuzz_stdlib`). Sale 0 si no hay discrepancias y 1 si hay alguna.
  Ejemplo: `python3 tools/fuzz/fuzz_stdlib.py --cases 3000 --seed 7 --suite all --jobs 6`.
  Las divergencias conocidas y aceptadas (enteros fuera de `Int64`, números no finitos en JSON, espacios o `_` en
  `parse_int`/`parse_float`) están explicadas en `docs/REAL_PROGRAMS.md`, ronda 23.

## Compilador Rust frente a selfhost

```sh
cargo build --release
YERUX_SEED=target/release/yerux bash selfhost/bootstrap.sh target/selfhost
python3 tools/fuzz/fuzz_selfhost.py --seed 233000 --count 2100 -j 8 --out /tmp/yerux-selfhost-fuzz
```

En Linux, muta programas independientes de `tests/` y `examples/` que el compilador Rust acepta, más los
generados por `fuzz_backends.py`. Usa los mutadores de `fuzz_valid.py` y `fuzz_syntax.py` y omite duplicados.
Compara el estado de `c` de ambos compiladores y, cuando rechazan, el primer mensaje principal, incluido el
código del diagnóstico si aparece. Cuando ambos generan C, lo compila con el mismo `cc -O0` y compara stdout
y el estado del ejecutable. Usa solo semillas locales sin dependencias del sistema y ejecuta en directorios
temporales con plazos. El arnés es manual y queda fuera de la suite.

`results.jsonl` tiene un registro por caso. Cada caso marcado conserva su fuente y sus diagnósticos en
`case-<id>/`; en un desacuerdo de ejecución conserva también las salidas. Pasa uno o más archivos
independientes como argumentos posicionales para repetir o reducir un hallazgo en un directorio de salida nuevo.
