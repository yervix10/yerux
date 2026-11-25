# Auditoría de `unwrap`/`expect`/`panic!` frente a `.yx` malformado

Revisión estática de `src/`, priorizando rutas de parseo, análisis y generación de código que reciben el AST derivado del fuente. No encontré cinco pánicos demostrablemente alcanzables desde un `.yx` malformado: los puntos siguientes son los cinco supuestos no comprobados más relevantes para probar. En cada caso, el flujo normal parece proteger el supuesto; no son defectos confirmados.

1. **`src/syntax/surface.rs:606` — `expect("surface match pattern")`.** La serialización del AST exige que cada brazo de `match` tenga patrón. Un patrón malformado podría causar pánico si un AST parcial llegara a esta fase; el parser de superficie actualmente devuelve error antes de serializarlo.
2. **`src/syntax/surface.rs:677` — `expect("surface generic arguments")`.** La serialización de una variante genérica exige argumentos presentes, incluso si están vacíos. Es un riesgo ante un AST parcial/inconsistente; no vi una ruta confirmada desde sintaxis inválida, que debe ser rechazada por el parser.
3. **`src/check_range.rs:426` — `self.lookup(&vname).unwrap()`.** El comprobador de conversiones a tipos de rango busca una variable que acaba de sintetizar mediante `self.chk`. Si el análisis de esa declaración falla sin detener el flujo, una conversión como `x as Int<0..10> else { ... }` podría alcanzar el `unwrap`; el código consulta primero el tipo con fallback, por lo que conviene verificar que los errores de `self.chk` siempre impiden continuar.
4. **`src/check.rs:2306` — `self.lookup(&vn).unwrap()`.** La expansión de `?` sintetiza una variable para el payload, la analiza y luego la busca. Una expresión `?` inválida podría ser problemática si el análisis registra un error pero no registra la variable; el nombre es interno y el flujo está diseñado para crearla antes de la búsqueda.
5. **`src/ir.rs:1642` — `pre_apre.take().unwrap()`.** La generación IR para `push_str`/`eq` presupone que el análisis previo preparó el argumento prestado. Una llamada mal tipada que escapara la barrera de errores del compilador podría terminar aquí; no encontré evidencia de que el flujo normal genere IR tras un error de tipos.

Los `unwrap` protegidos inmediatamente por `is_some`/validaciones de aridad y los pánicos de invariantes de backend no se priorizaron. No se modificó código del compilador.

## Fuzzing del frontend (`yerux check`)

Fuzzer propio (Python, fuera del repo): parte de `tests/*.yx`, `examples/*.yx` y `examples/real/**/*.yx` y aplica 1-3 mutaciones por
entrada: truncar, duplicar/borrar/permutar tokens, borrar o insertar llaves y comillas, intercambiar identificadores, sustituir
palabras clave y tipos, números enormes (`99999999999999999999999999`, `0xFFFF...`, `1e999999`), Unicode raro (BOM, RLO, NUL, controles,
emoji, U+10FFFF), anidamiento de 100 a 20000 niveles y líneas duplicadas o borradas. Cada entrada se ejecuta con `yerux check`
(plazo 5 s); si pasa, con `DEEP=1` también `yerux c` y `yerux ir`. Se busca: salida por señal o código >= 101, `panicked at src/`,
desbordamiento de pila, cuelgue y salida de error sin texto.

Medido: unas 190 000 entradas (7 semillas por 4 000 a 25 000 y 3 de 20 000 con `c`/`ir`), más 18 entradas hechas a mano (1 000 000 de
paréntesis, corchetes, llaves, `-` y `!` seguidos; cadena de 50 MB; 300 000 `let`; 200 000 brazos de `match`; enum de 100 000
variantes). Hallazgos, todos corregidos (3 pánicos o cuelgues y 1 defecto de presentación):

1. **Pánico** `index out of bounds` en `src/syntax.rs:951` (`intern_array`): `[0; 9223372036854775807]`. Una longitud literal >= `1 << 60`
   coincidía con los marcadores de parámetro const (`CONST_LEN + pid`). Ahora `array_len` la rechaza con «this array is too large».
   Prueba: `tests/e1981_array_len_huge.yx`.
2. **Cuelgue cuadrático** en enums con miles de variantes (8 000 variantes: 13 s; 16 000: más de 20 s). Tres causas:
   `check.rs` clonaba el `EnumDef` entero por cada brazo de `match` (el `==` automático de un enum sin payload genera un `match` por
   variante); `modules.rs::rewrite` recorría los tokens anteriores en cada identificador para contar paréntesis; y `cgen` indentaba con
   un nivel por anidamiento (un `match` de n brazos sobre enteros anida n niveles: espacios cuadráticos). Ahora: sin clon,
   profundidad de paréntesis precalculada, sangría limitada a 40 niveles. 8 000 variantes: 0,9 s; 100 000: 2 s (antes 86 s en
   `rewrite`).
3. **Límites con diagnóstico** para lo que sigue siendo superlineal: más de 5 000 variantes en un enum y más de 10 000 brazos en un
   `match` (`this enum has more than 5000 variants`, `this match has more than 10000 arms`). Pruebas:
   `tests/e1982_enum_too_many_variants.yx`, `tests/e1983_match_too_many_arms.yx`.
4. **Diagnóstico de 1 MB**: un error en una línea muy larga imprimía la línea entera (y el subrayado). Ahora se muestra una ventana
   de unos 160 caracteres alrededor de la columna, con `...` en los cortes (líneas de hasta 200 caracteres: sin cambio).

Sin hallazgos en el resto: lexer, parser, `check`, `c` e `ir` no entraron en pánico ni se colgaron con las demás mutaciones; el
anidamiento profundo ya tenía el límite de 2 000 niveles y las cadenas de operadores el de 50 000.

## Fuzzing profundo, ronda 2 (mutaciones sintácticamente válidas)

Segundo fuzzer propio (Python, fuera del repo): parte de `tests/*.yx` y `examples/*.yx` (sin los `e*.yx` ni archivos grandes) y aplica 1-3
mutaciones que suelen conservar la sintaxis: literales enteros por valores límite (`0`, `-1`, `127`, `128`, `255`, `256`, `65535`,
`2147483647`, `2147483648`, `4294967296`, `9223372036854775807`), operadores por otro de su familia (aritméticos, comparación, lógicos,
bit a bit), tipos por otro tipo numérico/`Bool`/`Text`, identificadores por otro de la misma entrada, y líneas borradas o duplicadas.
Cada mutante pasa por `yerux check`; si es aceptado, también `yerux c` e `yerux ir`. Se busca señal/código >= 101, `panicked at`,
cuelgue (10 s) y error sin texto.

Diferencial: de los mutantes aceptados se construye `yerux build` (C) y `yerux build --native`, se ejecutan con stdin vacío (8 s) y se
comparan código de salida y stdout (el C ya se compila con las comprobaciones habituales; ASan/UBSan lo cubre `tests/run_all.sh`).

Medido: 6 000 mutantes en `check`/`c`/`ir` y 683 mutantes aceptados en el diferencial C vs nativo. Resultado: sin pánicos, cuelgues ni
salidas sin texto en el frontend. Una única diferencia, ya documentada en SPEC.md (recursión infinita: el C acaba en SIGSEGV, el
nativo en `panic: stack overflow` con código 101; el oráculo C no tiene guardia). Sin defectos nuevos, por lo que no hay regresiones que añadir.

## Fuzzing profundo, ronda 14 (ASan/UBSan + fmt)

Herramientas en `tools/fuzz/` (ver su README). Campañas: 24 000 entradas malformadas con `check` + `fmt --stdout`
(sin panics, cuelgues, ni fmt no idempotente), y 3 × 2 500 mutaciones válidas con el C generado compilado con
`-fsanitize=address,undefined` y comparado con el backend nativo. Solo se sanitizó el C generado, no el compilador Rust.

- **Arreglado:** `link ""` (o con espacios / `-` inicial) llegaba al compilador de C como un `-l` suelto y fallaba con
  `cc: error: missing argument to '-l'`. Ahora es un diagnóstico del parser (`tests/e988_link_empty_name.yx`).
- **No son bugs:** fugas de LSan tras un `panic` (exit 101, el proceso aborta sin liberar; el fuzzer ya las ignora),
  `native_linux_memory`/`ptr_rw`/`pe_ffi` (mal uso de punteros crudos por la mutación, UB del propio programa),
  `generic_recursion` (recursión infinita: C sin guarda de pila, documentado en SPEC.md), `loop_dead_step`
  (bucle de ~9e18 iteraciones vacío: gcc lo elimina, el nativo lo ejecuta).
